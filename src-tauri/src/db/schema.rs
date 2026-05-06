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
