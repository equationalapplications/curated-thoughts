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
  SELECT CASE d.tier
    WHEN 'user_doc' THEN 'tier_fact'
    WHEN 'wiki' THEN 'tier_wisdom'
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

INSERT OR IGNORE INTO schema_version (version) VALUES (5);
";
