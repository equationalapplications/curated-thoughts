use crate::db::okf_ddl;
use crate::db::schema::{
    MIGRATION_V1, MIGRATION_V10, MIGRATION_V11, MIGRATION_V12, MIGRATION_V13, MIGRATION_V2,
    MIGRATION_V3, MIGRATION_V4, MIGRATION_V5, MIGRATION_V6, MIGRATION_V9,
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
    if version < 10 {
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V10))?;
    }
    if version < 11 {
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V11))?;
    }
    if version < 7 {
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", okf_ddl::migration_v7_sql()))?;
    }
    // V12 must run AFTER the OKF V7 DDL because it touches
    // `llm_wiki_entries.deleted_at`, which V7 creates. The V7/V8 gates
    // above were intentionally ordered to land before any data-migration
    // SQL; V12 follows the same convention.
    if version < 12 {
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))?;
    }
    if version < 13 {
        // `documents` predates this migration, so the column add is done
        // through the additive helper rather than inside the SQL constant.
        crate::db::ddl_compat::add_column_if_missing(
            conn,
            "documents",
            "quarantined_at",
            "INTEGER",
        )?;
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V13))?;
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
    /// Open the brain database, deriving the config path from the canonical
    /// resolver (honors `CURATED_BRAIN_DB` / `CURATED_BRAIN_CONFIG`).
    /// All new callers should use [`AppDb::open_with_config`] directly so the
    /// config path is explicit; this thin wrapper preserves the historical
    /// single-arg API while routing through the unified resolver.
    pub fn open(path: &Path) -> Result<Self> {
        let paths = crate::retrieval::resolve_brain_paths();
        Self::open_with_config(path, &paths.config_path)
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

/// Open (and migrate) a brain database at an arbitrary path. Intended for
/// tests that need an on-disk database file — production code must use
/// [`AppDb::open_with_config`] so the config-derived vault root is honored.
/// The `config` argument is accepted for API symmetry with `AppDb::open` and
/// is currently unused.
#[allow(dead_code)]
pub fn open_app_db(path: &Path, _config: Option<&Path>) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout = 5000;")?;
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
        assert_eq!(max_version, 13);
    }

    /// Fresh-DB path: `open_in_memory` applies every migration, so the
    /// watermark columns and the dirty-doc partial index must exist.
    #[test]
    fn migration_v11_fresh_db_adds_watermark_columns_and_dirty_index() {
        let conn = open_in_memory().unwrap();
        for column in &["synth_hash", "synth_model", "synth_at"] {
            let has_column: bool = conn
                .prepare("PRAGMA table_info(documents)")
                .unwrap()
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .filter_map(Result::ok)
                .any(|name| name == *column);
            assert!(
                has_column,
                "documents.{column} must exist on a fresh database"
            );
        }
        let index_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type='index' AND name='idx_documents_dirty'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(index_count, 1, "idx_documents_dirty must exist");
    }

    /// V12 idempotent unit-contract: every seconds-valued `deleted_at` row is
    /// promoted to ms on the first run, and a second run on the same data is
    /// a no-op. Pin the boundary against the 11-zeros off-by-one bug from
    /// spec review: rows that already pass `SEC_VS_MS_THRESHOLD` (i.e. were
    /// written in ms by the post-fix heal writers, or by `commit.rs:733` /
    /// `facts.rs:232`) must NOT be multiplied.
    #[test]
    fn migration_v12_promotes_seconds_and_is_idempotent() {
        use crate::db::schema::SEC_VS_MS_THRESHOLD;

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        // Build the schema to V11 so we have an `llm_wiki_entries` table to
        // mutate before V12 fires.
        conn.execute_batch(&format!(
            "BEGIN;\n{}\n{}\n{}\nCOMMIT;",
            MIGRATION_V1, MIGRATION_V2, MIGRATION_V3
        ))
        .unwrap();
        conn.execute_batch(MIGRATION_V4).unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V5))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V6))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", okf_ddl::migration_v7_sql()))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V9))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V10))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V11))
            .unwrap();

        // Seed three rows: one in seconds (the bug), one already in ms
        // (anything from `commit.rs:733` or `facts.rs:232`), one null.
        let insert = |deleted_at: Option<i64>| -> i64 {
            conn.execute(
                "INSERT INTO llm_wiki_entries
                    (id, entity_id, title, body, tags, confidence, source_type,
                     created_at, updated_at, deleted_at)
                 VALUES ('f' || hex(randomblob(6)), 'e', 't', 'b', '[]', 'inferred',
                         'librarian_inferred', 1, 1, ?1)",
                rusqlite::params![deleted_at],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        let secs_id = insert(Some(1_750_000_000)); // seconds — must be promoted
        let ms_id = insert(Some(SEC_VS_MS_THRESHOLD + 1)); // already ms — must NOT change
        let null_id = insert(None); // untouched

        // Pre-V12: confirm the seeded values.
        assert_eq!(
            conn.query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE rowid = ?1",
                [secs_id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap(),
            1_750_000_000,
            "seeded seconds-valued row must read back as seconds pre-V12"
        );

        // Fire V12.
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))
            .unwrap();

        let read = |id: i64| -> Option<i64> {
            conn.query_row(
                "SELECT deleted_at FROM llm_wiki_entries WHERE rowid = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(
            read(secs_id),
            Some(1_750_000_000 * 1000),
            "seconds-valued row must be promoted to milliseconds"
        );
        assert_eq!(
            read(ms_id),
            Some(SEC_VS_MS_THRESHOLD + 1),
            "already-ms row above threshold must NOT be multiplied"
        );
        assert_eq!(read(null_id), None, "NULL row must stay NULL");

        // Idempotency: re-running V12 must change nothing.
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V12))
            .unwrap();
        assert_eq!(read(secs_id), Some(1_750_000_000 * 1000));
        assert_eq!(read(ms_id), Some(SEC_VS_MS_THRESHOLD + 1));
        assert_eq!(read(null_id), None);

        // Pin the post-condition: zero rows below the threshold (the live-DB
        // smoke gate).
        let below: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_wiki_entries
                  WHERE deleted_at IS NOT NULL
                    AND deleted_at < ?1",
                [SEC_VS_MS_THRESHOLD],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(below, 0, "no row may remain below SEC_VS_MS_THRESHOLD");
    }

    /// Upgraded-DB path: simulate a pre-V11 database (schema_version = 10,
    /// no watermark columns), run the production `migrate` gate, and assert
    /// the columns appear.
    #[test]
    fn migration_v11_upgrades_v10_database() {
        // Pre-seed a V10 database: V1..V6 + OKF V7 DDL + data migration to 8 + V9 + V10.
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
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V9))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V10))
            .unwrap();

        let pre_has_synth_hash: bool = conn
            .prepare("PRAGMA table_info(documents)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .any(|name| name == "synth_hash");
        assert!(
            !pre_has_synth_hash,
            "test precondition: no synth_hash before migrate()"
        );

        migrate(&conn, None).expect("migrate must succeed upgrading a V10 DB");

        let post_has_synth_hash: bool = conn
            .prepare("PRAGMA table_info(documents)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .any(|name| name == "synth_hash");
        assert!(post_has_synth_hash, "synth_hash must exist after migrate()");
        let post_version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(post_version >= 11, "schema_version must reach >= 11");
    }

    /// Backfill correctness: a doc whose latest ingest run succeeded gets
    /// synth_hash = hash and synth_model = 'pre-watermark'; a doc with no
    /// ingest history (or whose latest run failed) stays NULL (dirty).
    #[test]
    fn migration_v11_backfills_only_docs_with_indexed_latest_run() {
        // Build a V10-state database by hand so we control ingest history.
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
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V9))
            .unwrap();
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V10))
            .unwrap();

        let insert_doc = |path: &str, hash: &str| -> i64 {
            conn.execute(
                "INSERT INTO documents (path, hash, tier, status) VALUES (?1, ?2, 'user_doc', 'indexed')",
                [path, hash],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        // Indexed-latest doc (older error, then indexed).
        let indexed_id = insert_doc("/v/a.md", "hash-a");
        conn.execute(
            "INSERT INTO ingest_runs (doc_id, run_at, outcome) VALUES (?1, 100, 'error')",
            [indexed_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ingest_runs (doc_id, run_at, outcome) VALUES (?1, 200, 'indexed')",
            [indexed_id],
        )
        .unwrap();
        // Error-latest doc.
        let errored_id = insert_doc("/v/b.md", "hash-b");
        conn.execute(
            "INSERT INTO ingest_runs (doc_id, run_at, outcome) VALUES (?1, 100, 'indexed')",
            [errored_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ingest_runs (doc_id, run_at, outcome) VALUES (?1, 300, 'error')",
            [errored_id],
        )
        .unwrap();
        // No-history doc.
        let _no_history_id = insert_doc("/v/c.md", "hash-c");

        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V11))
            .unwrap();

        let (a_hash, a_model): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT synth_hash, synth_model FROM documents WHERE id = ?1",
                [indexed_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            a_hash.as_deref(),
            Some("hash-a"),
            "indexed-latest doc must be backfilled with its hash"
        );
        assert_eq!(a_model.as_deref(), Some("pre-watermark"));

        {
            let id = errored_id;
            let (h, m): (Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT synth_hash, synth_model FROM documents WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert!(
                h.is_none(),
                "doc {id} without an indexed latest run must stay dirty"
            );
            assert!(m.is_none());
        }
        let dirty_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE synth_hash IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            dirty_count, 2,
            "exactly the error-latest and no-history docs stay dirty"
        );
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
    fn migration_v13_creates_watchdog_tables_and_quarantine_column() {
        let conn = open_in_memory().unwrap();

        for table in ["pipeline_heartbeat", "pipeline_stalls", "stall_strikes"] {
            let found: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "missing table {table}");
        }

        // documents gains a nullable quarantine timestamp.
        conn.execute_batch(
            "INSERT INTO documents (path, hash, tier, status, quarantined_at)
         VALUES ('/tmp/a.md', 'h', 'user_doc', 'pending', 123);",
        )
        .unwrap();
        let q: Option<i64> = conn
            .query_row(
                "SELECT quarantined_at FROM documents WHERE path = '/tmp/a.md'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(q, Some(123));

        // Heartbeat is a single-row table seeded at migration time.
        let hb: i64 = conn
            .query_row("SELECT COUNT(*) FROM pipeline_heartbeat", [], |r| r.get(0))
            .unwrap();
        assert_eq!(hb, 1);
    }

    #[test]
    fn migration_v13_is_idempotent() {
        let conn = open_in_memory().unwrap();
        // Re-running the migration body must not error or duplicate the seed row.
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", MIGRATION_V13))
            .unwrap();
        let hb: i64 = conn
            .query_row("SELECT COUNT(*) FROM pipeline_heartbeat", [], |r| r.get(0))
            .unwrap();
        assert_eq!(hb, 1);
    }

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
