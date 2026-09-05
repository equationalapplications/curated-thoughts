//! DDL for OKF tables: package-owned `llm_wiki_*` (verbatim from core-llm-wiki setupDatabase)
//! plus Rust-owned `curated_*` staging tables.

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
  embedding_attempts INTEGER NOT NULL DEFAULT 0
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

CREATE INDEX IF NOT EXISTS llm_wiki_edges_entity_idx ON llm_wiki_edges(entity_id);

CREATE TABLE IF NOT EXISTS llm_wiki_events (
  id TEXT PRIMARY KEY,
  entity_id TEXT NOT NULL,
  event_type TEXT NOT NULL,
  summary TEXT NOT NULL,
  related_entry_id TEXT,
  created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS llm_wiki_events_entity_idx ON llm_wiki_events(entity_id, created_at DESC);

CREATE TABLE IF NOT EXISTS llm_wiki_checkpoints (
  entity_id TEXT PRIMARY KEY,
  heal_checkpoint INTEGER NOT NULL DEFAULT 0,
  memory_checkpoint INTEGER NOT NULL DEFAULT 0
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
