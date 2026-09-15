# Document deletion: restore shadow-copy removal and wiki-page orphaning

**Date:** 2026-09-15
**Status:** Proposed (rev 1)
**Branch:** spec/211-deletion-side-effects
**Issue:** #211
**Priority:** Medium (disk litter in `.brain/converted` + derived wiki pages never marked `orphaned`; no index corruption)

## Problem

Until `2ed0acf` swapped the in-process pipeline for the DB-backed
`enqueue_vault_event`, deletions flowed through `PipelineJob::Delete`, whose
worker arm did three things: delete the `documents` row, remove the shadow
copy at `.brain/converted/<stem>.md` (PDF/DOCX conversion artifact), and mark
wiki pages derived from the document as `orphaned`. `2ed0acf` removed the
producers; `95f6636` (clippy enforcement) then removed the unreachable
consumer arm — and with it `tests/deletion.rs`, which asserted both side
effects. Neither side effect ever migrated to the DB-queue delete path.

Today every production code path that deletes a `documents` row skips both
side effects:

| Site | Trigger |
|---|---|
| `db/queue.rs:100` — Remove branch of `enqueue_vault_event` | watcher Remove event |
| `db/queue.rs:157` — NotFound branch | Add/Modify whose file vanished before read (out-of-order delete) |
| `reconcile.rs:192` — excluded pre-pass | reconcile: row moved under an excluded dir |
| `reconcile.rs:218` — vanished `None` branch | reconcile: file gone, no rename match |
| `reconcile.rs:259` — `purge_brain_rows` | reconcile: empty-walk `.brain` heal |
| `lib.rs:842` — `purge_excluded_rows` | startup heal of excluded-directory rows |

(`db/proposals.rs:818` also deletes a row but is test-only; out of scope.)

Consequences:

1. **Shadow copies accumulate.** Nothing in-tree removes from
   `.brain/converted`; every reference creates or tests the directory
   (`lib.rs:610`, `vault/layout.rs:29`).
2. **Stale derived wiki pages.** Deleting a source document leaves every
   wiki page generated from it in `pending_review`/`approved` forever. The
   only other `'orphaned'` write is the one-off OKF migration
   (`db/okf_migration.rs:129`).

The old arm's orphan matching was also imprecise:
`source_doc_ids LIKE '%<path>%'` matches any path that contains the deleted
path as a substring — deleting `/v/a.md` orphans a page sourced from
`/v/a.md.orig`.

## Design

### D1 — Unified helper `delete_document_with_cleanup`

Add to `db/queries.rs`, absorbing `delete_document` (`db/queries.rs:117`),
which has had no production callers since `95f6636`:

```rust
pub fn delete_document_with_cleanup(
    conn: &Connection,
    path: &str,
    vault_root: Option<&Path>,
) -> Result<usize>
```

Behavior, in order:

1. `DELETE FROM documents WHERE path = ?1`; the helper returns **this**
   statement's affected-row count (not the orphan UPDATE's), so
   `purge_excluded_rows` keeps its count.
2. The wiki-page orphan UPDATE from D2.
3. Best-effort shadow-copy removal per D3.

The helper takes `&Connection` so it composes with a caller's open
transaction (`rusqlite::Transaction` derefs to `Connection`): reconcile's
row-delete and orphaning commit or roll back together. It opens no
transaction of its own.

### D2 — Precise orphan matching via `json_each`

Replace the old substring `LIKE` with exact JSON-array element matching:

```sql
UPDATE wiki_pages SET status = 'orphaned'
WHERE status NOT IN ('rejected', 'orphaned')
  AND json_valid(source_doc_ids)
  AND EXISTS (SELECT 1 FROM json_each(wiki_pages.source_doc_ids)
              WHERE value = ?1)
```

- `source_doc_ids` is JSON `TEXT` (default `'[]'`); the review shim writes
  an array of source **path strings** (`db/review_shim.rs:46`).
- `json_valid` guard: a malformed legacy row is skipped, not an error — the
  same fail-soft posture as `tier_backfill`'s guard (`db/connection.rs:758`).
  Without it, one bad row would abort the whole delete path mid-`UPDATE`.
- Legacy OKF rows hold `Vec<i64>` document ids (`db/okf_migration.rs:136`);
  an integer never equals a path string, so they are never matched — the
  same outcome as the old `LIKE`, no regression.
- Semantics are strictly those of the removed arm (decision, 2026-09-15):
  **any** listed source deleted → page flips to `orphaned`, even if other
  sources remain. "Only when the last source dies" was rejected to avoid
  introducing new read-modify-write behavior never shipped before.
- `status NOT IN ('rejected', 'orphaned')` keeps the old guard: rejected
  pages keep their verdict; the UPDATE is idempotent.
- `wiki_pages.status` CHECK admits `'orphaned'` since schema V3
  (`db/schema.rs:94`); no schema change is needed.

### D3 — Shadow-copy removal, best-effort

Derive the shadow path exactly as the removed arm did:

- stem: `Path::new(path).file_stem()`
- root: `vault_root` when the caller knows it; otherwise fall back to
  `path.parent().and_then(|p| p.parent())` (documents live one level under
  the vault root). Skip silently when neither yields a root or the path has
  no file stem.
- target: `<root>/.brain/converted/<stem>.md`
- `let _ = std::fs::remove_file(target)` — missing file and IO errors are
  ignored, matching the old arm. Nothing in-tree currently writes these
  artifacts; the cleanup is defensive for conversion artifacts and planted
  files.

Known limitation (pre-existing, unchanged): the shadow name is the bare
stem, so `a.pdf` and `a.docx` (or same-named files in different
directories) share one shadow path; deleting either removes it.

### D4 — Rewire the production delete sites

| Site | Change |
|---|---|
| `db/queue.rs` Remove branch | call helper; wrap the call in `conn.unchecked_transaction()` so delete + orphan commit atomically (the branch currently runs a single bare statement) |
| `db/queue.rs:157` NotFound branch | same as Remove branch |
| `reconcile.rs` main pass (excluded pre-pass + vanished `None` branch) | call helper on the open `tx` — atomicity already provided |
| `reconcile.rs` `purge_brain_rows` | call helper on the open `tx` |
| `lib.rs` `purge_excluded_rows` | call helper (keeps summing returned counts); no transaction — `&Connection`, startup heal retried next launch, unchanged posture |

`enqueue_vault_event` takes `&mut Connection`, so `unchecked_transaction`
is available at both queue sites. `vault_root` plumbing: queue sites pass
`configured_root.as_deref()`; reconcile and `purge_excluded_rows` already
hold a required `vault_root: &Path`.

### D5 — Tests

Restore `src-tauri/tests/deletion.rs` (deleted by `95f6636`) in spirit,
targeting the DB-queue path:

1. **Happy path:** seed a document, plant `.brain/converted/<stem>.md`,
   insert a wiki page whose `source_doc_ids` JSON array contains the
   document path; fire `enqueue_vault_event` with `EventKind::Remove`;
   assert the `documents` row is gone, the shadow file is gone, and the
   page status is `orphaned`.
2. **Prefix imprecision:** deleting `/v/a.md` must NOT orphan a page whose
   `source_doc_ids` contains only `/v/a.md.orig`.
3. **Guard:** a page already `rejected` (and one already `orphaned`) keeps
   its status.
4. **Legacy integers:** a page with `source_doc_ids = "[1,2]"` is not
   orphaned by a path delete.
5. **Malformed JSON:** a page with non-JSON `source_doc_ids` is skipped and
   the delete still succeeds.
6. **Reconcile path:** a vanished-file reconcile deletion orphans the page
   and removes the shadow copy (exercises the helper inside a
   transaction).
7. **No shadow planted:** deletion succeeds when no shadow file exists
   (best-effort removal must not error).

## Non-goals

- Last-source-dies orphan semantics (rejected above).
- Outbox propagation of the `orphaned` status flip to prisma-outbox
  replicas — same umbrella as the `wiki_forget` outbox gap (#132); tracked
  there, not here.
- Any change to how shadow copies are written (nothing in-tree writes
  them).
- `db/proposals.rs:818` (test-only raw delete).
