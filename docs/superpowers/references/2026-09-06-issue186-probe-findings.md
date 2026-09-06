# Issue #186 probe findings — live brain measurement (Task 1)

Date: 2026-09-06
Method: verbatim per `.superpowers/sdd/2026-09-06-issue186-evidence-source-ref-writer/task-1-brief.md`, run against a copy at `/tmp/issue186-probe/brain.db` (source `~/.brain/brain.db`, 1,064,960 bytes, SQLite WAL mode, WAL file 0 bytes at copy time so the copy is complete). The live DB was never opened for writing. sqlite3 3.43.2 (macOS system build) — JSON1 verified working (`json_valid`/`json_extract` both functional) before running the metrics.

## Result: the live brain contains ZERO wiki data

Every metric from Step 2 returned 0. This is not a query problem — the tables exist (full `llm_wiki_*` schema present, `schema_version` = 16) but are empty.

| metric | n |
|---|---|
| total_librarian_inferred | 0 |
| null_ref | 0 |
| valid_json_ref | 0 |
| shape_evidenceproposal_id | 0 |
| shape_evidencechunk_id | 0 |
| len_255 | 0 |
| outbox_entries_rows | 0 |
| outbox_recoverable | 0 |

Full-database row census confirms emptiness is not confined to the wiki tables: `llm_wiki_entries` 0, `llm_wiki_outbox` 0, `wiki_pages` 0, `llm_wiki_source_ref_index` 0, `llm_wiki_edges` 0, `llm_wiki_tasks` 0, `llm_wiki_events` 0, `chunks` 0, `embeddings` 0, `curated_entities` 0. The only non-empty tables are `schema_version` (16), `llm_wiki_meta` (1), `pipeline_heartbeat` (1), `system_strikes` (1), and `documents` (3 — all ingest `error` rows, no successful ingest).

`~/.brain/config.json` corroborates: `privacy.mode` is `"ephemeral"`, `vault_path` points into a temp dir (`/var/folders/.../T/.tmp63dThd/vault`), and `migrated_to_v2` is false. There is no alternative brain DB anywhere under `~/Library/Application Support`.

## Step 3 samples

Not runnable in any meaningful sense — the damaged-shape population is empty, so there is nothing to sample:

```
sqlite3 /tmp/issue186-probe/brain.db "SELECT substr(source_ref,1,80) ... NOT json_valid(source_ref) LIMIT 5;"
→ (no rows)
```

**The Step 3 stop condition is vacuously untriggered but cannot be confirmed either.** No sampled ref begins with `proposal_id`, because there are no sampled refs at all. The §2.5.4 key-order assumption (mangled refs read `evidence` first because serde_json emits keys alphabetically) remains plausible but is **unverified against real data on this machine**.

## Scope decision (Step 4)

**`outbox_recoverable` = 0 of 0 damaged rows — coverage is indeterminable, not ≥95% and not <95%.** Per the Step 4 rubric this lands on the conservative branch: **4b/4c must be budgeted as if they carry real load**, and Task 6's fixtures must cover both real mangled shapes (`evidenceproposal_id…` and `evidencechunk_id…`, including the `length(source_ref)=255` truncation signature) since no real damaged rows exist to mirror. Path 4a (outbox-first) should still be implemented per spec — the Rust source never deletes outbox rows, so the recovery path is sound — but it cannot be validated against production data here.

## Implications for the plan

1. **No measurement of real damage is possible on this machine.** If issue #186 was observed against a different brain (another machine, a non-ephemeral profile), the probe must be re-run against that DB copy before Task 2+ locks its repair budget. Ask the user where the affected brain lives.
2. All repair-path fixtures and tests must be **fully synthetic** (Task 6 note above) — there is no real corpus to replay.
3. The probe copy is retained at `/tmp/issue186-probe/brain.db` for re-measurement; nothing was written to `~/.brain/brain.db`.
