//! DDL for OKF tables: package-owned `llm_wiki_*` (verbatim from core-llm-wiki setupDatabase)
//! plus Rust-owned `curated_*` staging tables.

use rusqlite::OptionalExtension;

/// Default table prefix for core-llm-wiki in Curated Thoughts.
pub const LLM_WIKI_PREFIX: &str = "llm_wiki_";

/// SQLite table name for package metadata (`llm_wiki_meta`).
pub const LLM_WIKI_META_TABLE: &str = "llm_wiki_meta";

/// Verbatim `setupDatabase` SQL from `@equationalapplications/core-llm-wiki` with prefix applied.
pub const LLM_WIKI_PACKAGE_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS llm_wiki_entries (
  id TEXT PRIMARY KEY,
  entity_id TEXT NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  tags TEXT NOT NULL DEFAULT '[]',
  confidence TEXT NOT NULL DEFAULT 'inferred',
  source_type TEXT NOT NULL DEFAULT 'librarian_inferred',
  source_hash TEXT,
  source_ref TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  last_accessed_at INTEGER,
  access_count INTEGER NOT NULL DEFAULT 0,
  deleted_at INTEGER,
  embedding TEXT,
  embedding_blob BLOB,
  okf_type TEXT,
  ontology_checked_at INTEGER,
  heal_checked_at INTEGER,
  lifecycle_status TEXT NOT NULL DEFAULT 'stable',
  stale_after INTEGER,
  generated_by TEXT,
  last_verified_at INTEGER,
  last_verified_by TEXT,
  okf_sources TEXT,
  okf_verified TEXT,
  okf_usage_window TEXT,
  embedding_failed_at INTEGER,
  embedding_failure_kind TEXT,
  embedding_attempts INTEGER NOT NULL DEFAULT 0,
  valid_from INTEGER,
  valid_to INTEGER,
  superseded_by TEXT,
  superseded_at INTEGER,
  prior_valid_to INTEGER,
  source_pointer TEXT,
  derived_events TEXT
);

CREATE INDEX IF NOT EXISTS llm_wiki_entries_entity_idx ON llm_wiki_entries(entity_id);
CREATE INDEX IF NOT EXISTS llm_wiki_entries_source_ref_idx ON llm_wiki_entries(entity_id, source_ref);
CREATE INDEX IF NOT EXISTS llm_wiki_entries_source_hash_idx ON llm_wiki_entries(entity_id, source_hash) WHERE source_hash IS NOT NULL;
CREATE INDEX IF NOT EXISTS llm_wiki_entries_updated_idx ON llm_wiki_entries(updated_at DESC);

-- source_ref_index: per-(entity, source_hash) record of the canonical sourceRef
-- currently holding that hash. The partial UNIQUE index on (entity_id, source_hash)
-- WHERE deleted_at IS NULL enforces the sourceRef-level TOCTOU-race invariant;
-- entries-level uniqueness cannot express it because a single ingestDocument call
-- writes N facts that all share (entity_id, source_ref, source_hash). See
-- docs/superpowers/specs/2026-08-07-dependabot-concurrency-release-hygiene-design.md \xA7B1.
CREATE TABLE IF NOT EXISTS llm_wiki_source_ref_index (
  id TEXT PRIMARY KEY,
  entity_id TEXT NOT NULL,
  source_hash TEXT NOT NULL,
  source_ref TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  deleted_at INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS llm_wiki_idx_source_ref_hash
  ON llm_wiki_source_ref_index (entity_id, source_hash)
  WHERE deleted_at IS NULL;

-- source_documents / source_passages: the source store (specs/source-store \xA73).
-- New tables only; hard delete, so no deleted_at and both UNIQUE indexes are plain.
-- Passage text is not stored: offsets (UTF-16 code units) slice source_documents.text.
CREATE TABLE IF NOT EXISTS llm_wiki_source_documents (
  id TEXT PRIMARY KEY,
  entity_id TEXT NOT NULL,
  source_ref TEXT NOT NULL,
  source_hash TEXT NOT NULL,
  text TEXT NOT NULL,
  text_hash TEXT NOT NULL,
  char_count INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS llm_wiki_source_passages (
  id TEXT PRIMARY KEY,
  entity_id TEXT NOT NULL,
  document_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  start_offset INTEGER NOT NULL,
  end_offset INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS llm_wiki_tasks (
  id TEXT PRIMARY KEY,
  entity_id TEXT NOT NULL,
  description TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending',
  priority INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  resolved_at INTEGER,
  deleted_at INTEGER,
  okf_type TEXT,
  lifecycle_status TEXT NOT NULL DEFAULT 'stable',
  stale_after INTEGER,
  generated_by TEXT,
  last_verified_at INTEGER,
  last_verified_by TEXT,
  okf_sources TEXT,
  okf_verified TEXT,
  okf_usage_window TEXT
);

CREATE INDEX IF NOT EXISTS llm_wiki_tasks_entity_idx ON llm_wiki_tasks(entity_id, status);

CREATE TABLE IF NOT EXISTS llm_wiki_edges (
  id TEXT PRIMARY KEY,
  entity_id TEXT NOT NULL,
  source_id TEXT NOT NULL,
  target_id TEXT NOT NULL,
  edge_type TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  UNIQUE(entity_id, source_id, target_id, edge_type)
);

CREATE INDEX IF NOT EXISTS llm_wiki_edges_entity_id_idx ON llm_wiki_edges(entity_id, id);

CREATE TABLE IF NOT EXISTS llm_wiki_events (
  id TEXT PRIMARY KEY,
  entity_id TEXT NOT NULL,
  event_type TEXT NOT NULL,
  summary TEXT NOT NULL,
  related_entry_id TEXT,
  created_at INTEGER NOT NULL,
  occurred_at INTEGER,
  forgotten_at INTEGER
);

CREATE INDEX IF NOT EXISTS llm_wiki_events_entity_idx ON llm_wiki_events(entity_id, created_at DESC);

CREATE TABLE IF NOT EXISTS llm_wiki_checkpoints (
  entity_id TEXT PRIMARY KEY,
  heal_checkpoint INTEGER NOT NULL DEFAULT 0,
  memory_checkpoint INTEGER NOT NULL DEFAULT 0,
  librarian_watermark_at INTEGER,
  librarian_watermark_id TEXT
);

CREATE TABLE IF NOT EXISTS llm_wiki_entity_manifests (
  entity_id TEXT PRIMARY KEY,
  mode TEXT NOT NULL DEFAULT 'off',
  manifest_json TEXT NOT NULL DEFAULT '{"node_types":[],"edge_types":[]}',
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS llm_wiki_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS llm_wiki_outbox (
  id TEXT PRIMARY KEY,
  entity_id TEXT NOT NULL,
  table_name TEXT NOT NULL,
  record_id TEXT NOT NULL,
  operation TEXT NOT NULL,
  payload TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS llm_wiki_outbox_entity_id_created_at
  ON llm_wiki_outbox (entity_id, created_at);

CREATE INDEX IF NOT EXISTS llm_wiki_outbox_created_at
  ON llm_wiki_outbox (created_at);
CREATE TABLE IF NOT EXISTS llm_wiki_vector_removals (
  seq             INTEGER PRIMARY KEY AUTOINCREMENT,
  entity_id       TEXT NOT NULL,
  fact_id         TEXT NOT NULL UNIQUE,
  enqueued_at     INTEGER NOT NULL,
  attempts        INTEGER NOT NULL DEFAULT 0,
  last_attempt_at INTEGER,
  confirmed_at    INTEGER
);
CREATE INDEX IF NOT EXISTS llm_wiki_vector_removals_pending
  ON llm_wiki_vector_removals (entity_id, attempts, seq) WHERE confirmed_at IS NULL;

CREATE TRIGGER IF NOT EXISTS llm_wiki_vector_removals_enqueue
AFTER UPDATE OF deleted_at ON llm_wiki_entries
WHEN OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL
BEGIN
  -- DELETE then INSERT, not INSERT OR REPLACE: an outer UPSERT's conflict
  -- handling would override OR REPLACE and abort on fact_id (spec rev 5).
  DELETE FROM llm_wiki_vector_removals WHERE fact_id = NEW.id;
  INSERT INTO llm_wiki_vector_removals (entity_id, fact_id, enqueued_at)
  VALUES (NEW.entity_id, NEW.id, CAST(strftime('%s','now') AS INTEGER) * 1000);
END;

-- A hard delete outside prune (host SQL, an older engine after a
-- downgrade). A live fact is owed only where an external index was in use;
-- a soft-deleted one drops the rows nothing can owe (spec rev 5).
CREATE TRIGGER IF NOT EXISTS llm_wiki_vector_removals_hard_delete
AFTER DELETE ON llm_wiki_entries
BEGIN
  DELETE FROM llm_wiki_vector_removals
   WHERE fact_id = OLD.id
     AND (OLD.deleted_at IS NULL
          OR confirmed_at IS NOT NULL
          OR NOT EXISTS (SELECT 1 FROM llm_wiki_meta WHERE key = 'external_index_in_use' AND value = '1'));
  INSERT INTO llm_wiki_vector_removals (entity_id, fact_id, enqueued_at)
  SELECT OLD.entity_id, OLD.id, CAST(strftime('%s','now') AS INTEGER) * 1000
   WHERE OLD.deleted_at IS NULL
     AND EXISTS (SELECT 1 FROM llm_wiki_meta WHERE key = 'external_index_in_use' AND value = '1');
END;
"#;

/// Rust-owned staging and local-only tables (spec §1).
pub const CURATED_TABLES_DDL: &str = r"
CREATE TABLE IF NOT EXISTS curated_entities (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    entity_type TEXT NOT NULL DEFAULT 'concept',
    summary TEXT NOT NULL DEFAULT '',
    summary_embedding BLOB,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER
);

CREATE TABLE IF NOT EXISTS curated_proposals (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind IN ('new_entity','update_entity')),
    entity_id TEXT,
    proposed_name TEXT,
    proposed_type TEXT,
    reasoning TEXT,
    model TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK(status IN ('pending','approved','rejected','partial','superseded')),
    reject_reason TEXT,
    created_at INTEGER NOT NULL,
    resolved_at INTEGER
);

CREATE TABLE IF NOT EXISTS curated_proposal_items (
    id TEXT PRIMARY KEY,
    proposal_id TEXT NOT NULL REFERENCES curated_proposals(id) ON DELETE CASCADE,
    item_type TEXT NOT NULL CHECK(item_type IN
        ('fact_add','fact_update','fact_archive','edge_add','task_add','summary_update')),
    target_id TEXT,
    payload TEXT NOT NULL,
    evidence TEXT NOT NULL DEFAULT '[]',
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK(status IN ('pending','accepted','rejected')),
    edited_payload TEXT
);

CREATE TABLE IF NOT EXISTS curated_proposal_sources (
    proposal_id TEXT NOT NULL REFERENCES curated_proposals(id) ON DELETE CASCADE,
    doc_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'evidence' CHECK(role IN ('trigger','evidence')),
    PRIMARY KEY (proposal_id, doc_id)
);

CREATE TABLE IF NOT EXISTS curated_agent_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    client TEXT NOT NULL,
    tool TEXT NOT NULL,
    operation TEXT NOT NULL CHECK(operation IN ('read','write')),
    entity_id TEXT,
    summary TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_curated_proposals_status ON curated_proposals(status);
CREATE INDEX IF NOT EXISTS idx_curated_proposal_items_proposal ON curated_proposal_items(proposal_id);
CREATE INDEX IF NOT EXISTS idx_curated_proposal_sources_doc ON curated_proposal_sources(doc_id);
CREATE INDEX IF NOT EXISTS idx_curated_agent_log_created ON curated_agent_log(created_at);
";

/// Full V7 schema migration SQL (package + curated tables).
pub fn migration_v7_sql() -> String {
    format!(
        "{}{}\nINSERT OR IGNORE INTO schema_version (version) VALUES (7);",
        LLM_WIKI_PACKAGE_DDL.trim(),
        CURATED_TABLES_DDL.trim(),
    )
}

/// Mirror of core-llm-wiki engine migration V12 (7.7.1): replace the
/// single-column `edges(entity_id)` index with the composite
/// `edges(entity_id, id)` index used by `lint()` keyset paging.
///
/// Runs on every Rust open so a brain first opened by the CLI/MCP binaries —
/// before any TS `setup()` — gets the same index shape. Create-before-drop
/// keeps `llm_wiki_edges` covered by an `entity_id` index throughout; both
/// statements are idempotent, so running after the engine's own V12 is a
/// no-op.
pub fn apply_llm_wiki_v12_edge_index(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS llm_wiki_edges_entity_id_idx ON llm_wiki_edges(entity_id, id);
         DROP INDEX IF EXISTS llm_wiki_edges_entity_idx;",
    )
}

/// Mirror of core-llm-wiki engine migration 13 (7.8.0) / `setupDatabase`'s
/// `createTemporalIndexesIfColumnsExist` (7.9.0): the two partial temporal
/// indexes on `llm_wiki_entries`.
///
/// These indexes are created OUTSIDE the engine's `setupDatabase` exec
/// template, so they must stay out of `LLM_WIKI_PACKAGE_DDL` — `ddl_compat`
/// compares template statement sets and would flag them as Rust-only drift.
/// Like the engine's own helper, each index is gated on its columns existing
/// (`PRAGMA table_info`), so this is safe before V24 has run and a no-op
/// after the engine's own migration 13 has. Runs on every Rust open so a
/// brain first opened by the CLI/MCP binaries gets the same index shape.
pub fn apply_llm_wiki_v13_temporal_indexes(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let existing = crate::db::ddl_compat::existing_columns(conn, "llm_wiki_entries")?;
    let has = |col: &str| existing.iter().any(|c| c == col);
    if has("superseded_by") {
        conn.execute(
            "CREATE INDEX IF NOT EXISTS llm_wiki_entries_superseded_idx
             ON llm_wiki_entries(entity_id, superseded_by) WHERE superseded_by IS NOT NULL",
            [],
        )?;
    }
    if has("valid_from") && has("valid_to") {
        conn.execute(
            "CREATE INDEX IF NOT EXISTS llm_wiki_entries_temporal_idx
             ON llm_wiki_entries(entity_id) WHERE valid_from IS NOT NULL OR valid_to IS NOT NULL",
            [],
        )?;
    }
    Ok(())
}

/// Mirror of core-llm-wiki 9.18.0 `setupDatabase`'s
/// `createOkfV02IndexesIfColumnsExist` (`dist/index.js:2247`): per
/// `llm_wiki_entries` / `llm_wiki_tasks`, the `lifecycle_status`,
/// `stale_after` and `last_verified_at` indexes, each gated on its column.
///
/// Created OUTSIDE the `setupDatabase` template; must stay out of
/// `LLM_WIKI_PACKAGE_DDL` (`ddl_compat` compares template statements only).
/// Runs on every Rust open, unstamped (v12/v13 precedent), so fresh and
/// migrated brains converge without waiting for a TS `setup()`.
pub fn apply_llm_wiki_okf_v02_indexes(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    for table in ["entries", "tasks"] {
        let existing =
            crate::db::ddl_compat::existing_columns(conn, &format!("{LLM_WIKI_PREFIX}{table}"))?;
        for col in ["lifecycle_status", "stale_after", "last_verified_at"] {
            if existing.iter().any(|c| c == col) {
                conn.execute(
                    &format!(
                        "CREATE INDEX IF NOT EXISTS {LLM_WIKI_PREFIX}{table}_{col}_idx ON {LLM_WIKI_PREFIX}{table}({col})"
                    ),
                    [],
                )?;
            }
        }
    }
    Ok(())
}

/// Mirror of core-llm-wiki 9.18.0 engine migrations v18/v19's partial
/// indexes — `createEntriesDeletedIndex` (`dist/index.js:2855`) and
/// `createEventsForgottenIndex` (`:2844`). Migration-only in the engine
/// (never in the `setupDatabase` template), so they must stay out of
/// `LLM_WIKI_PACKAGE_DDL`. `entries_deleted_idx` is unconditional in the
/// engine (`deleted_at` is a template column); `events_forgotten_idx` is gated
/// on `forgotten_at` existing (the engine adds the column first, v19).
/// Every open, unstamped.
pub fn apply_llm_wiki_v18_v19_partial_indexes(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute(
        "CREATE INDEX IF NOT EXISTS llm_wiki_entries_deleted_idx ON llm_wiki_entries(entity_id, deleted_at) WHERE deleted_at IS NOT NULL",
        [],
    )?;
    let events = crate::db::ddl_compat::existing_columns(conn, "llm_wiki_events")?;
    if events.iter().any(|c| c == "forgotten_at") {
        conn.execute(
            "CREATE INDEX IF NOT EXISTS llm_wiki_events_forgotten_idx ON llm_wiki_events(entity_id, forgotten_at) WHERE forgotten_at IS NOT NULL",
            [],
        )?;
    }
    Ok(())
}

/// Mirror of core-llm-wiki 9.18.0 `setupDatabase`'s
/// `createSourcePassageIndexesIfColumnsExist` (`dist/index.js:2278`) and
/// `createSourceDocumentIndexesIfColumnsExist` (`:2286`): the two passage
/// indexes (gated on `document_id` + `entity_id`) and the two UNIQUE document
/// indexes (gated on `entity_id` + `source_ref` + `source_hash`). Created
/// OUTSIDE the `setupDatabase` template; must stay out of
/// `LLM_WIKI_PACKAGE_DDL`. Every open, unstamped.
pub fn apply_llm_wiki_source_store_indexes(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let passages = crate::db::ddl_compat::existing_columns(conn, "llm_wiki_source_passages")?;
    let p = |c: &str| passages.iter().any(|x| x == c);
    if p("document_id") && p("entity_id") {
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS llm_wiki_idx_source_passages_doc ON llm_wiki_source_passages (document_id, seq);
    CREATE INDEX IF NOT EXISTS llm_wiki_idx_source_passages_entity ON llm_wiki_source_passages (entity_id);",
        )?;
    }
    let documents = crate::db::ddl_compat::existing_columns(conn, "llm_wiki_source_documents")?;
    let d = |c: &str| documents.iter().any(|x| x == c);
    if d("entity_id") && d("source_ref") && d("source_hash") {
        conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS llm_wiki_idx_source_documents_ref ON llm_wiki_source_documents (entity_id, source_ref);
    CREATE UNIQUE INDEX IF NOT EXISTS llm_wiki_idx_source_documents_hash ON llm_wiki_source_documents (entity_id, source_hash);",
        )?;
    }
    Ok(())
}

/// Mirror of core-llm-wiki 9.18.0 `setupDatabase`'s `createResurrectTrigger`
/// (`dist/index.js:2219-2246`, level ladder `resurrectLevel` `:2215`):
/// `llm_wiki_vector_removals_resurrect`, `AFTER UPDATE OF deleted_at ON
/// llm_wiki_entries` (column-scoped — the PR-A re-type tripwire holds).
/// Body level: 0 = no vector columns (only drops the ledger row); 1 = also
/// clears `embedding_blob`/`embedding` when the removal was confirmed; 2 =
/// also resets the embed-failure marker columns. An existing trigger at a
/// lower level than the columns allow is dropped and recreated; an equal or
/// higher one is left alone (the engine's rule). SQL text is the engine's
/// template with the prefix applied. Created OUTSIDE the `setupDatabase`
/// template; must stay out of `LLM_WIKI_PACKAGE_DDL`. Every open, unstamped.
pub fn apply_llm_wiki_resurrect_trigger(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    const NAME: &str = "llm_wiki_vector_removals_resurrect";
    fn level(sql: &str) -> u8 {
        if sql.contains("embedding_failed_at") {
            2
        } else if sql.contains("embedding_blob") {
            1
        } else {
            0
        }
    }
    let cols = crate::db::ddl_compat::existing_columns(conn, "llm_wiki_entries")?;
    let has = |c: &str| cols.iter().any(|x| x == c);
    let want: u8 = if !(has("embedding_blob") && has("embedding")) {
        0
    } else if ["embedding_failed_at", "embedding_failure_kind", "embedding_attempts"]
        .iter()
        .all(|c| has(c))
    {
        2
    } else {
        1
    };
    let existing: Option<Option<String>> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
            [NAME],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(sql) = existing {
        if level(sql.as_deref().unwrap_or("")) >= want {
            return Ok(());
        }
        conn.execute_batch(&format!("DROP TRIGGER IF EXISTS {NAME};"))?;
    }
    let clear_vector = match want {
        0 => String::new(),
        _ => format!(
            "
      UPDATE llm_wiki_entries
         SET embedding_blob = NULL, embedding = NULL{}
       WHERE id = NEW.id
         AND EXISTS (SELECT 1 FROM llm_wiki_vector_removals WHERE fact_id = NEW.id AND confirmed_at IS NOT NULL);",
            if want == 2 {
                ",
             embedding_failed_at = NULL, embedding_failure_kind = NULL, embedding_attempts = 0"
            } else {
                ""
            }
        ),
    };
    conn.execute_batch(&format!(
        "
    CREATE TRIGGER IF NOT EXISTS {NAME}
    AFTER UPDATE OF deleted_at ON llm_wiki_entries
    WHEN OLD.deleted_at IS NOT NULL AND NEW.deleted_at IS NULL
    BEGIN{clear_vector}
      DELETE FROM llm_wiki_vector_removals WHERE fact_id = NEW.id;
    END;
  "
    ))
}
