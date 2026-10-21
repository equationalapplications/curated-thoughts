-- Task 1 (user-confirmation-queue plan; spec rev 4.1 §1/D4): deposit-time
-- attribution index. The deposit note's OKF frontmatter is the AUTHORITATIVE
-- carrier; this table is a derived brain-side cache keyed by document path,
-- written inside the same audit_and_seed_kick transaction as the audit and
-- kick-ledger rows (wisdom_deposit.rs). A re-ingest repopulates the index
-- from the file.
--
-- Idempotent DDL executed on every open — no schema_version stamp (the
-- V18/V26 "DDL only" precedent). Task 2 extends migrations/ with its own
-- files (stated_attestations, queue_denials); do not add them here.
CREATE TABLE IF NOT EXISTS deposit_attribution (
  document_path TEXT PRIMARY KEY,
  speaker       TEXT NOT NULL,
  kind          TEXT NOT NULL,
  source        TEXT NOT NULL,
  deposited_at  TEXT NOT NULL
);
