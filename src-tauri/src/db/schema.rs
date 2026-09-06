// `embeddings` BLOB table (V2). Vector ANN (e.g. sqlite-vec) is a future perf path; see `search` module docs.

/// Boundary that separates `llm_wiki_entries.deleted_at` values written in
/// milliseconds (`>=` this) from the historical seconds-valued rows (`<`
/// this). Twelve zeros, not eleven: 1_000_000_000_000 ms = 2001-09-09,
/// 100_000_000_000 (11 zeros) = 1973-03-03 in seconds. The off-by-one-zero
/// variant was the bug caught in spec review for the V12 migration; both
/// constants are pinned by U1/U2.
pub const SEC_VS_MS_THRESHOLD: i64 = 1_000_000_000_000;
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
                    CHECK(status IN ('pending', 'pending_reindex', 'indexed', 'error', 'orphaned'))
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

/// Heal-writer timestamp-unit migration. Two writers in `lib.rs` were writing
/// `deleted_at = unixepoch()` (seconds) while every other `llm_wiki_entries`
/// writer used milliseconds (`commit.rs:733`, `facts.rs:232`, etc.). This
/// migration multiplies any seconds-valued `deleted_at` by 1000 so the column
/// is uniformly milliseconds after the heal writers are fixed.
///
/// Boundary: `deleted_at < SEC_VS_MS_THRESHOLD` (1_000_000_000_000 = 2001-09-09
/// in ms). Every real-world row, present or future, is well past that in ms
/// or well below it in seconds, so the heuristic is safe. Idempotent: a
/// second run finds zero rows to update because the first run already pushed
/// all candidates above the threshold.
pub const MIGRATION_V12: &str = "
-- Promote seconds-valued soft-deletes to milliseconds so the timestamp-unit
-- audit (curated-thoughts-runwikiheal-bug-2026-08-26 §C) sees a single
-- convention across the column.
UPDATE llm_wiki_entries
   SET deleted_at = deleted_at * 1000
 WHERE deleted_at IS NOT NULL
   AND deleted_at < 1000000000000;

INSERT OR IGNORE INTO schema_version (version) VALUES (12);
";

pub const MIGRATION_V13: &str = "
-- Watchdog state. Spec: docs/superpowers/specs/2026-08-31-ingest-drain-stall-watchdog-design.md §2.4
-- Single-row mirror of the in-memory heartbeat, so worker state survives to
-- post-mortem and is readable by the headless CLI.
CREATE TABLE IF NOT EXISTS pipeline_heartbeat (
    id               INTEGER PRIMARY KEY CHECK (id = 1),
    epoch            INTEGER NOT NULL DEFAULT 0,
    seq              INTEGER NOT NULL DEFAULT 0,
    stage            TEXT    NOT NULL DEFAULT 'idle',
    subject          TEXT,
    stage_started_ms INTEGER NOT NULL DEFAULT 0,
    updated_ms       INTEGER NOT NULL DEFAULT 0
);
INSERT OR IGNORE INTO pipeline_heartbeat (id) VALUES (1);

-- One row per watchdog trip. Written BEFORE any recovery action (§3).
CREATE TABLE IF NOT EXISTS pipeline_stalls (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    tripped_ms    INTEGER NOT NULL,
    kind          TEXT    NOT NULL,   -- 'stage_stall' | 'drain_stall'
    stage         TEXT    NOT NULL,
    subject       TEXT,
    stalled_ms    INTEGER NOT NULL,
    heartbeat_seq INTEGER NOT NULL,
    epoch         INTEGER NOT NULL,
    pending_count INTEGER NOT NULL,
    embed_endpoint TEXT,
    gen_endpoint   TEXT,
    action        TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_pipeline_stalls_tripped ON pipeline_stalls(tripped_ms);

-- Strike ledger keyed by document path (§4.2).
CREATE TABLE IF NOT EXISTS stall_strikes (
    path        TEXT PRIMARY KEY,
    strikes     INTEGER NOT NULL DEFAULT 0,
    last_ms     INTEGER NOT NULL
);

INSERT OR IGNORE INTO schema_version (version) VALUES (13);
";

pub const MIGRATION_V14: &str = "
-- Single-row ledger for unattributed system strikes. Used when the stall is
-- caused by a shared local dependency (e.g. brain SQLite contention under
-- Committing/Deleting) so that no innocent document inherits blame (§4.2).
CREATE TABLE IF NOT EXISTS system_strikes (
    id       INTEGER PRIMARY KEY CHECK (id = 1),
    strikes  INTEGER NOT NULL DEFAULT 0,
    last_ms  INTEGER NOT NULL
);
INSERT OR IGNORE INTO system_strikes (id, strikes, last_ms) VALUES (1, 0, 0);

INSERT OR IGNORE INTO schema_version (version) VALUES (14);
";

pub const MIGRATION_V15: &str = "
-- Widen `documents.status` to admit 'pending_reindex'.
--
-- `queue_full_reindex` and `run_wiki_reembed` stage rows as 'pending_reindex'
-- when the pipeline channel is full, so the §5 sweep can re-enqueue them as a
-- forced rechunk. The V1 CHECK constraint never listed that value, so every
-- staging UPDATE failed with a constraint violation and the deferred work was
-- silently dropped — the exact data loss the deferral exists to prevent.
--
-- SQLite cannot ALTER a CHECK constraint, so the table is rebuilt with the
-- full column set as of V14: the V1 columns, the V11 synthesis watermark
-- (synth_hash/synth_model/synth_at) and the V13 quarantine stamp. Columns are
-- listed explicitly rather than via SELECT * so the copy does not depend on
-- the order the ALTERs appended them in. `idx_documents_dirty` is dropped
-- with the old table and recreated against the new one.
PRAGMA foreign_keys = OFF;

DROP INDEX IF EXISTS idx_documents_dirty;

CREATE TABLE documents_v15 (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    path            TEXT    NOT NULL UNIQUE,
    hash            TEXT    NOT NULL,
    tier            TEXT    NOT NULL CHECK(tier IN ('user_doc', 'wiki')),
    folder_rules_id INTEGER,
    last_indexed    INTEGER,
    status          TEXT    NOT NULL DEFAULT 'pending'
                    CHECK(status IN ('pending', 'pending_reindex', 'indexed', 'error', 'orphaned')),
    synth_hash      TEXT,
    synth_model     TEXT,
    synth_at        INTEGER,
    quarantined_at  INTEGER
);

INSERT INTO documents_v15
    (id, path, hash, tier, folder_rules_id, last_indexed, status,
     synth_hash, synth_model, synth_at, quarantined_at)
SELECT id, path, hash, tier, folder_rules_id, last_indexed, status,
       synth_hash, synth_model, synth_at, quarantined_at
  FROM documents;

DROP TABLE documents;
ALTER TABLE documents_v15 RENAME TO documents;

CREATE INDEX IF NOT EXISTS idx_documents_dirty
    ON documents(status) WHERE synth_hash IS NULL;

PRAGMA foreign_keys = ON;

INSERT OR IGNORE INTO schema_version (version) VALUES (15);
";

/// Tier as a stored dimension on wiki entries.
///
/// `'fact'` is anchor truth (the librarian must not propose modifications),
/// `'wisdom'` is curated and proposal-updatable, NULL is working/unclassified
/// — the posture of every entry that exists at migration time.
///
/// The CHECK is the invariant's floor. A bare `TEXT NULL` accepts any string,
/// and an entry with an out-of-vocabulary tier would carry no prompt semantics
/// and match no filter. Write boundaries validate the same set so callers get
/// a diagnostic instead of a constraint violation, but the database is the
/// authority. Every existing row is NULL, so no data pass is needed.
///
/// Spec: `2026-09-01-memory-architecture-intent-implementation-design.md` §3.1.
pub const MIGRATION_V16: &str = "
ALTER TABLE llm_wiki_entries
    ADD COLUMN tier TEXT NULL
    CHECK (tier IN ('fact', 'wisdom') OR tier IS NULL);

CREATE INDEX IF NOT EXISTS idx_llm_wiki_entries_tier
    ON llm_wiki_entries(tier) WHERE tier IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (16);
";

/// V18: CT-owned evidence storage for librarian-inferred facts (issue #186).
///
/// Structured evidence moves out of `llm_wiki_entries.source_ref` — which the
/// JS engine's `setup()` rewrites unconditionally through `normalizeSourceRef`,
/// destroying any JSON in it — and into this table, which the engine never
/// touches. `source_ref` keeps only a normalizer-idempotent token.
///
/// The `json_valid` CHECK is the guardrail: JSON is universally required in
/// this column, so any future mangling fails loudly at write time instead of
/// silently corrupting provenance.
///
/// The FK CASCADE documents intent only. SQLite enforces foreign keys per
/// connection (`PRAGMA foreign_keys=ON`) and brain.db has several connections
/// whose pragma state we do not control, so every deletion path issues an
/// explicit paired `DELETE FROM librarian_evidence`. See spec §2.1.
/// V18 DDL only — deliberately carries **no** schema_version stamp. The V18
/// one-shot repair (connection.rs) runs after this DDL and stamps version 18
/// itself, so a crash mid-repair leaves the database unstamped and the next
/// open re-enters the migration and retries. Every statement here is
/// idempotent (`IF NOT EXISTS`) to make that re-entry safe. Spec §2.5.
pub const MIGRATION_V18: &str = "
CREATE TABLE IF NOT EXISTS librarian_evidence (
  entry_id      TEXT PRIMARY KEY REFERENCES llm_wiki_entries(id) ON DELETE CASCADE,
  proposal_id   TEXT NOT NULL,
  evidence_json TEXT NOT NULL CHECK(json_valid(evidence_json)),
  unanchored    INTEGER NOT NULL DEFAULT 0,
  created_at    INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS librarian_evidence_proposal_idx
  ON librarian_evidence(proposal_id);
";

/// The complete stored-tier vocabulary for `llm_wiki_entries.tier`.
///
/// The V16 CHECK is the database-level floor; this is the same set expressed
/// once for every write boundary above it, so a caller gets a diagnostic
/// instead of a constraint violation. Adding a tier means editing the CHECK in
/// [`MIGRATION_V16`] and this slice together — nothing else hard-codes the set.
pub const VALID_TIERS: &[&str] = &["fact", "wisdom"];

/// Whether `tier` is a value the V16 CHECK will admit as non-NULL.
///
/// NULL (working/unclassified) is represented by `Option::None` at every
/// boundary rather than by a string, so it is deliberately not a member here.
pub fn is_valid_tier(tier: &str) -> bool {
    VALID_TIERS.contains(&tier)
}
