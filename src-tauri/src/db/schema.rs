// embeddings table is omitted here — added in Sub-project 2 when sqlite-vec is integrated.
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
