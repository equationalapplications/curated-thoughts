-- Task 2 (user-confirmation-queue plan; spec rev 4.1 §8): stated verification
-- tables. `stated_attestations` mirrors the app-written
-- stated/.attestations.jsonl ledger (content-hash keyed, one row per attested
-- file); `queue_denials` mirrors stated/.denials.jsonl and pins suppression by
-- (document path, chunk hash, fact-text hash). Both are DERIVED brain-side
-- indexes rebuilt by db::stated_ledger::rebuild_rederive — the vault JSONL
-- ledgers are the durable carriers.
--
-- Idempotent DDL executed on every open — no schema_version stamp (the same
-- ungated shape as 0001_deposit_attribution; the repo-wide precedent is the
-- V27/V28 bodies, which also run on every open).
CREATE TABLE IF NOT EXISTS stated_attestations (
  document_path TEXT PRIMARY KEY,
  content_hash  TEXT NOT NULL,
  attested_by   TEXT NOT NULL,
  attested_at   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS queue_denials (
  document_path  TEXT NOT NULL,
  chunk_hash     TEXT NOT NULL,
  fact_text_hash TEXT NOT NULL,
  denied_by      TEXT NOT NULL,
  denied_at      TEXT NOT NULL,
  PRIMARY KEY (document_path, chunk_hash, fact_text_hash)
);
