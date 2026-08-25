// `embeddings` BLOB table (V2). Vector ANN (e.g. sqlite-vec) is a future perf path; see `search` module docs.
pub const MIGRATION_V1: &str = "
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS documents (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    path            TEXT    NOT NULL UNIQUE,
    hash            TEXT    NOT NULL,
    tier            TEXT    NOT NULL CHECK(tier IN ('user_doc', 'wiki')),
    folder_rules_id INTEGER,
    last_indexed    INTEGER,
    status          TEXT    NOT NULL DEFAULT 'pending'
                    CHECK(status IN ('pending', 'indexed', 'error', 'orphaned'))
);

CREATE TABLE IF NOT EXISTS chunks (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    doc_id     INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    chunk_text TEXT    NOT NULL,
    position   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS wiki_pages (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    path           TEXT    NOT NULL UNIQUE,
    source_doc_ids TEXT    NOT NULL DEFAULT '[]',
    generated_by   TEXT    NOT NULL,
    last_synced    INTEGER,
    status         TEXT    NOT NULL DEFAULT 'pending_review'
                   CHECK(status IN ('pending_review', 'approved', 'rejected'))
);

CREATE TABLE IF NOT EXISTS folder_rules (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    folder_path     TEXT    NOT NULL UNIQUE,
    librarian_mode  TEXT    NOT NULL DEFAULT 'index'
                    CHECK(librarian_mode IN ('index', 'summarize', 'synthesize')),
    provider_override TEXT,
    model_override    TEXT,
    auto_approve      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY
);

INSERT OR IGNORE INTO schema_version (version) VALUES (1);
";

pub const MIGRATION_V2: &str = "
CREATE TABLE IF NOT EXISTS embeddings (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    chunk_id INTEGER NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    vector   BLOB    NOT NULL
);

INSERT OR IGNORE INTO schema_version (version) VALUES (2);
";

pub const MIGRATION_V3: &str = "
CREATE TABLE IF NOT EXISTS wiki_pages_new (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    path           TEXT    NOT NULL UNIQUE,
    source_doc_ids TEXT    NOT NULL DEFAULT '[]',
    generated_by   TEXT    NOT NULL,
    last_synced    INTEGER,
    status         TEXT    NOT NULL DEFAULT 'pending_review'
                   CHECK(status IN ('pending_review', 'approved', 'rejected', 'orphaned'))
);

INSERT OR IGNORE INTO wiki_pages_new SELECT * FROM wiki_pages;
DROP TABLE wiki_pages;
ALTER TABLE wiki_pages_new RENAME TO wiki_pages;

INSERT OR IGNORE INTO schema_version (version) VALUES (3);
";

pub const MIGRATION_V4: &str = "
ALTER TABLE chunks ADD COLUMN start_line   INTEGER NOT NULL DEFAULT 1;
ALTER TABLE chunks ADD COLUMN end_line     INTEGER NOT NULL DEFAULT 1;
ALTER TABLE chunks ADD COLUMN symbol_name  TEXT;
ALTER TABLE chunks ADD COLUMN strategy     TEXT NOT NULL DEFAULT 'prose';

INSERT OR IGNORE INTO schema_version (version) VALUES (4);
";

pub const MIGRATION_V5: &str = "
ALTER TABLE chunks ADD COLUMN defined_symbol TEXT DEFAULT NULL;
ALTER TABLE chunks ADD COLUMN entity_id TEXT;

UPDATE chunks SET entity_id = (
  SELECT CASE
    WHEN d.path LIKE '%/documents/%' THEN 'tier_fact'
    WHEN d.path LIKE '%/wiki/%' THEN 'tier_wisdom'
    ELSE 'tier_working'
  END
  FROM documents d WHERE d.id = chunks.doc_id
);

CREATE INDEX IF NOT EXISTS idx_chunks_defined_symbol
    ON chunks (defined_symbol, entity_id)
    WHERE defined_symbol IS NOT NULL;

CREATE TABLE IF NOT EXISTS curated_relationships (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    from_id     INTEGER NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    to_id       INTEGER NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    rel_type    TEXT    NOT NULL,
    symbol      TEXT    NOT NULL,
    entity_id   TEXT    NOT NULL,
    created_at  INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_rel_symbol
    ON curated_relationships (symbol, entity_id);

CREATE INDEX IF NOT EXISTS idx_rel_to_id
    ON curated_relationships (to_id, entity_id);

CREATE INDEX IF NOT EXISTS idx_rel_from_id
    ON curated_relationships (from_id, entity_id);

CREATE UNIQUE INDEX IF NOT EXISTS idx_rel_unique
    ON curated_relationships (from_id, to_id, rel_type);

INSERT OR IGNORE INTO schema_version (version) VALUES (5);
";

pub const MIGRATION_V6: &str = "
INSERT OR IGNORE INTO schema_version (version) VALUES (6);
";

pub const MIGRATION_V9: &str = "
ALTER TABLE chunks ADD COLUMN content_hash TEXT NOT NULL DEFAULT '';

-- Partial unique index: dedup by real hash, but allow multiple placeholder
-- (empty) hashes per doc. Empty `content_hash` rows are pre-migration test
-- fixtures / backfill-skip rows; real ingest always writes a non-empty hash.
CREATE UNIQUE INDEX IF NOT EXISTS idx_chunks_doc_hash
    ON chunks(doc_id, content_hash) WHERE content_hash != '';

INSERT OR IGNORE INTO schema_version (version) VALUES (9);
";

pub const MIGRATION_V10: &str = "
-- Per-doc ingest history: documents.last_indexed is overwritten on every
-- re-ingest, so temporal questions ('when did X last change BEFORE today?')
-- were unanswerable. This table preserves one row per ingest attempt.
CREATE TABLE IF NOT EXISTS ingest_runs (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    doc_id  INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    run_at  INTEGER NOT NULL DEFAULT (unixepoch()),
    outcome TEXT    NOT NULL CHECK(outcome IN ('indexed', 'error'))
);

CREATE INDEX IF NOT EXISTS idx_ingest_runs_doc_time
    ON ingest_runs (doc_id, run_at);

INSERT OR IGNORE INTO schema_version (version) VALUES (10);
";

pub const MIGRATION_V11: &str = "
-- Synthesis watermark columns: track which document hash was last
-- summarized by which model, so unchanged docs can be skipped.
ALTER TABLE documents ADD COLUMN synth_hash TEXT;
ALTER TABLE documents ADD COLUMN synth_model TEXT;
ALTER TABLE documents ADD COLUMN synth_at INTEGER;

CREATE INDEX idx_documents_dirty
    ON documents(status) WHERE synth_hash IS NULL;

-- Best-effort backfill: a doc whose most recent ingest attempt succeeded
-- predates watermarking entirely, so its current hash must have been what
-- was last synthesized. Mark it clean without pretending any model ran.
UPDATE documents
   SET synth_hash = hash,
       synth_model = 'pre-watermark'
 WHERE EXISTS (
     SELECT 1 FROM ingest_runs ir
      WHERE ir.doc_id = documents.id
        AND ir.outcome = 'indexed'
        AND ir.run_at = (
            SELECT MAX(ir2.run_at) FROM ingest_runs ir2
             WHERE ir2.doc_id = documents.id
         )
 );

INSERT OR IGNORE INTO schema_version (version) VALUES (11);
";
