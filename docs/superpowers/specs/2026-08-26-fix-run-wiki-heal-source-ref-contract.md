# Spec: Fix `run_wiki_heal` source-ref consumer + lock timestamp unit contracts

**Date:** 2026-08-26
**Branch:** `fix/run-wiki-heal-source-ref-contract` (forked from `origin/main` @ `d284c16`)
**Status:** DRAFT — awaiting Kurt's review
**Type:** bug fix + hardening
**Vision tie-in:** the heal path is the second-most-load-bearing piece of librarian
maintenance (after ingest). The timestamp-unit contract is cross-cutting — a single
inconsistency silently corrupts every "recent" query in the app.

## Context

Two independent bugs surfaced in the handoff at `~/hermes/handoff/curated-thoughts-runwikiheal-bug-2026-08-26.md`
plus a third that came out of the unit-contract investigation:

### Bug A (handoff): `run_wiki_heal` misreads `source_ref` as a vault path
- Producer (`src-tauri/src/db/commit.rs:524`, since commit `c30f141` 2026-08-20) writes
  JSON `{"proposal_id":..., "evidence":[{"chunk_id":...}]}` into `source_ref`.
- Consumer (`src-tauri/src/lib.rs:1379–1421` and the dual `heal_invalid_sources` at
  `:356–430`) treats it as a vault-relative path under `PathMode::MustExist`. Every
  `librarian_inferred` row with the new JSON `source_ref` fails the existence check.
- Unit test `lib.rs:3449` was not updated and still uses the old path-shaped contract,
  so the test passes green while production silently soft-deletes every live row.

### Bug B (this spec, also from handoff): the auto-heal fires on vault delete events
- `spawn_heal_scheduler` (`lib.rs:432`) consumes `heal_tx.send(())` events emitted by
  the watcher at `lib.rs:965` on any `VaultEvent::Deleted`. So even if the user never
  clicks "Heal" in the UI, **deleting a vault file triggers a heal run**.
- `heal_invalid_sources` (`:356`) has the same path-vs-JSON bug AND lacks the
  `source_type = 'librarian_inferred'` filter, so it would also wipe `user_stated` rows
  whose `source_ref` is the `MANUAL_SOURCE_REF` sentinel from
  `db/facts.rs:14` (`{"proposal_id":null,"evidence":[]}`).
- This is the more dangerous of the two — the watcher can fire it without user action.

### Bug C (this spec, surfaced by unit-contract audit): mixed seconds/milliseconds writers
On the live DB at `~/.brain-equational-wiki/brain.db` I confirmed the two
already-damaged rows have `deleted_at = 1787658855640` (milliseconds), not seconds
(`unixepoch()` returns seconds). That means the killer was NOT `run_wiki_heal`; it was
something else writing ms. Audit:

| Column | Writer | Unit |
|---|---|---|
| `llm_wiki_entries.deleted_at` | `lib.rs:400` (`heal_invalid_sources`) | **seconds** ⚠ |
| `llm_wiki_entries.deleted_at` | `lib.rs:1414` (`heal_lost_librarian_inferred`) | **seconds** ⚠ |
| `llm_wiki_entries.deleted_at` | `commit.rs:733` (`commit_fact_archive`) | ms |
| `llm_wiki_entries.deleted_at` | `facts.rs:232` (`archive_fact`) | ms |
| `llm_wiki_entries.created_at`/`updated_at` | `commit.rs:524` | ms |
| `llm_wiki_tasks.deleted_at` | `tasks.rs:305` | ms |
| `llm_wiki_events.created_at` | `commit.rs:996` | ms |
| `llm_wiki_outbox.created_at` | `commit.rs` outbox writers | ms |
| `curated_entities.created_at`/`updated_at` | `commit.rs:450` | seconds ✓ (schema default is `unixepoch()`) |
| `curated_entities.deleted_at` | `entities.rs:563` | seconds ✓ (matches schema default) |
| `documents.last_indexed` | `linker.rs:237`, `queries.rs:93` | seconds (schema has no default) |
| `documents.last_indexed` reader | `events.rs:113` multiplies by `* 1000` to ms for display | cross-unit fix already in place |
| `documents.synth_at` | `synthesis.rs:910` | seconds |
| `ingest_runs.run_at` | schema default `unixepoch()` | seconds |

The two `heal_*` writers in `lib.rs` are the only outliers. All other write/read pairs
agree. The simplest, most surgical fix is to bring those two in line with the
milliseconds convention used everywhere else for `llm_wiki_entries`.

This audit also confirms why the `coerce_updated_at` helper in `tools/src/queries.rs:152`
exists — the recall-sidecar PR #82 was already a band-aid for a downstream symptom
of the same root issue (text-vs-int). We're now closing it upstream.

## Goals

- Bug A + B: every live `librarian_inferred` row with a JSON `source_ref` is preserved
  by heal runs (both manual and watcher-triggered).
- Bug B+: the watcher heal stops wiping `user_stated` and `immutable_document` rows.
- Bug C: every `llm_wiki_entries.deleted_at` value written from this PR onward is in
  milliseconds. The two pre-existing seconds-valued soft-deleted rows in the live DB
  are migrated to milliseconds via `MIGRATION_V12`.
- Future regressions of the same class are caught by a contract test that runs on every
  PR.

## Non-goals

- Refactoring `llm_wiki_events.created_at` writers (they're already correct ms).
- Normalizing `curated_entities` to ms (schema default and writers all agree on seconds;
  changing it would require a separate, broader contract change with implications for
  the OKF bundle format and MCP wire payload).
- Touching the `coerce_updated_at` helper — it's already a defensive layer for
  historical data; the new contract test makes it less needed over time.
- Re-architecting the heal scheduler. Keeping the watcher-triggered heal (with the bug
  fixed) preserves its intended "self-healing vault" behavior.

## Design

### 1. New helper: `source_ref_is_still_grounded` in `src-tauri/src/db/commit.rs`

```rust
/// Returns true iff `source_ref` represents a chunk/fact that still exists in the
/// vault. `source_ref` can be either a vault-relative path (legacy producer
/// contract) or the JSON `{"proposal_id":..., "evidence":[...]}` shape produced
/// by `evidence_json_with_hashes` since commit c30f141.
///
/// Empty / null / parse-error → returns `true` (no-op). The heal policy is
/// "soft-delete if the reference is *demonstrably* stale", and a row that
/// can't be parsed isn't demonstrably stale — it's a legacy path or a future
/// producer we don't know about yet. Logging is the right response, not
/// deletion. (This is the contract that all five new tests lock in.)
pub(crate) fn source_ref_is_still_grounded(
    conn: &Connection,
    source_ref: &str,
) -> bool
```

Logic:
1. If `source_ref` doesn't start with `{`, treat as legacy vault-relative path;
   existence-check against `documents.path = ?1` with `status='indexed'`. If no
   document matches → return false.
2. Parse as JSON. If parse fails → return true (defensive: see comment).
3. Extract `evidence[*].chunk_id`. If no chunk_ids present (e.g. `MANUAL_SOURCE_REF`
   with `evidence:[]`) → return true (user_stated is not librarian-grounded).
4. For each chunk_id, check existence in `chunks` table. If **all** chunk_ids are
   missing → return false. If **any** is alive → return true (a fact with
   partial evidence is still partially grounded).
5. Empty `evidence[]` array → return true (matches `MANUAL_SOURCE_REF` semantics).

### 2. Fix both consumers to use the new helper

`heal_lost_librarian_inferred` (`lib.rs:1379`):
```rust
for (rowid, source_ref) in entries {
    if !crate::db::commit::source_ref_is_still_grounded(conn, &source_ref) {
        updated += conn.execute(
            "UPDATE llm_wiki_entries SET deleted_at = ?1 WHERE rowid = ?2",
            params![ms_now(), rowid],  // <-- now_ms, not unixepoch()
        )?;
    }
}
```

`heal_invalid_sources` (`lib.rs:356`):
```rust
// Add: scope to source_types that are *expected* to be grounded
// in vault chunks. user_stated and immutable_document are user-authored
// or non-vault — they don't go through this contract.
WHERE e.deleted_at IS NULL
  AND e.source_ref IS NOT NULL
  AND e.source_type = 'librarian_inferred'
```

Also switch the `deleted_at =` setter to `ms_now()` (Bug C fix), and only write
`llm_wiki_events` rows for entities that actually had entries soft-deleted (the
current code does this already, good).

### 3. New helper: `ms_now() -> i64` colocated with `now_timestamps()`

```rust
pub(crate) fn ms_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
```

Use everywhere `lib.rs` previously used `unixepoch()` for `deleted_at`. (No other
`unixepoch()` writers need to change — they're all correct.)

### 4. Migration: `MIGRATION_V12` in `src-tauri/src/db/schema.rs`

```rust
// In src-tauri/src/db/schema.rs
pub const SEC_VS_MS_THRESHOLD: i64 = 1_000_000_000_000; // 12 zeros, 2001-09-09 in ms

pub const MIGRATION_V12: &str = "
-- Backfill: legacy heal runs wrote deleted_at as unixepoch() (seconds)
-- into llm_wiki_entries. Every other writer uses milliseconds. Multiply
-- any value still in seconds so future readers (prune, query) compare
-- apples-to-apples. Heuristic: a timestamp is in seconds iff it is below
-- SEC_VS_MS_THRESHOLD (12 zeros = 2001-09-09 in ms). CRITICAL: the literal
-- here must have exactly 12 zeros. 11 zeros = 1973-03-03 in seconds and
-- would skip every modern seconds timestamp (incl. the 2 known damaged
-- rows on the live DB). Tested by U-test (see below).
UPDATE llm_wiki_entries
SET deleted_at = deleted_at * 1000
WHERE deleted_at IS NOT NULL
  AND deleted_at < 1000000000000;
";
```

Wired in `src-tauri/src/db/connection.rs:migrate()` after V11. Idempotent: a row
already in ms will exceed `SEC_VS_MS_THRESHOLD` and be left alone. The threshold
is exposed as a const so the U-test can import and assert against the same
constant — no chance of the SQL literal and the test diverging by a digit.

**A regression test in `src-tauri/tests/timestamp_units.rs` asserts the constant
value, not just behavior:**
```rust
#[test]
fn migration_v12_threshold_constant_is_12_zeros() {
    // Direct guard against the off-by-one-zero bug caught in spec review
    // (Aug 26 2026: PR #99 review). 11 zeros (100000000000) = 1973-03-03
    // in seconds — would migrate nothing useful. 12 zeros is correct.
    assert_eq!(
        crate::db::schema::SEC_VS_MS_THRESHOLD,
        1_000_000_000_000,
        "SEC_VS_MS_THRESHOLD must have exactly 12 zeros (2001-09-09 in ms)"
    );
    // 11 zeros must NOT match (regression lock):
    assert_ne!(
        crate::db::schema::SEC_VS_MS_THRESHOLD,
        100000000000,
        "11 zeros would skip all modern seconds timestamps"
    );
}
```

A second statement for `ingest_runs.run_at` would be wrong: its schema default
is `unixepoch()` and consumers compare in seconds, so it's already correct.

### 5. New test module: `src-tauri/src/db/commit.rs::tests::source_ref_grounding`

Five tests, all use `open_in_memory()`:

- **D1. JSON source_ref with alive chunk → returns true (heal skips)**
  Insert a chunk, write a fact via `evidence_json_with_hashes`, assert
  `source_ref_is_still_grounded` returns true.

- **D2. JSON source_ref with all-dead chunks → returns false (heal soft-deletes)**
  Insert a chunk, write a fact, then `DELETE FROM chunks WHERE id=?`. Assert
  the function now returns false.

- **D3. Legacy path source_ref pointing at an existing indexed document → true**
  Insert a document, write a fact with `source_ref = "documents/foo.md"`.
  Assert true.

- **D4. Legacy path source_ref pointing at a missing document → false**
  Same as D3 but no document inserted.

- **D5. MANUAL_SOURCE_REF sentinel (`{"proposal_id":null,"evidence":[]}`) → true**
  The empty-evidence case. This locks in the contract that user_stated rows
  are never healed-away regardless of producer.

### 6. New test module: `src-tauri/src/lib.rs::tests::heal_unit_contracts`

- **E1. heal_lost_librarian_inferred preserves JSON source_refs with alive chunks**
  Refactor of the existing `heal_soft_deletes_missing_librarian_inferred_entries_only`
  at `lib.rs:3449`. New contract: insert a fact with JSON `source_ref` referencing
  a real chunk; call heal; assert no soft-delete.

- **E2. heal_lost_librarian_inferred soft-deletes JSON source_refs with all-dead chunks**
  Same setup as E1, but `DELETE FROM chunks` first. Assert soft-delete happens.

- **E3. heal_invalid_sources is scoped to librarian_inferred**
  Insert one `librarian_inferred` row with a missing-vault path, one
  `user_stated` row with `MANUAL_SOURCE_REF`, and one `immutable_document` row.
  Call `heal_invalid_sources`. Assert: only the librarian_inferred row is
  touched. (Regression for the missing `source_type` filter that today would
  wipe user_stated rows.)

- **E4. Both heal writers set deleted_at in milliseconds (not seconds)**
  For each heal function, call it on a soft-delete-eligible row, then
  assert `deleted_at >= 1_000_000_000_000` (any ms timestamp after 2001-09-09).
  This is the **direct regression test for Bug C** in this PR.

- **E5. Mixed seconds/ms rows in the live DB are migrated by MIGRATION_V12**
  Insert two rows: one with `deleted_at = 1_700_000_000` (seconds), one with
  `deleted_at = 1_700_000_000_000` (ms). Run the migration manually. Assert
  the seconds row was multiplied; the ms row was not touched. (Idempotency
  check.)

### 7. New top-level test module: `src-tauri/tests/timestamp_units.rs`

This is the **comprehensive contract matrix** the user asked for. One test per
column from the table above. Each test:
1. Opens an in-memory DB.
2. Performs the action that writes the column.
3. Reads it back and asserts the unit (via threshold + comparison to a known
   `as_secs()` / `as_millis()` value).

Tests:
- **U1. `llm_wiki_entries.deleted_at` from `archive_fact` is in ms**
- **U2. `llm_wiki_entries.deleted_at` from `commit_fact_archive` is in ms**
- **U3. `llm_wiki_entries.deleted_at` from `heal_lost_librarian_inferred` is in ms** (post-fix)
- **U4. `llm_wiki_entries.deleted_at` from `heal_invalid_sources` is in ms** (post-fix)
- **U5. `llm_wiki_tasks.deleted_at` from `archive_task` is in ms**
- **U6. `llm_wiki_tasks.resolved_at` from `update_task_status` is in ms**
- **U7. `llm_wiki_events.created_at` from `record_event` is in ms**
- **U8. `llm_wiki_outbox.created_at` from outbox writers is in ms**
- **U9. `llm_wiki_entries.created_at`/`updated_at` from a fact_add commit is in ms**
- **U10. `curated_entities.deleted_at` from `archive_entity` is in seconds**
- **U11. `curated_entities.created_at` from `create_entity` is in seconds**
- **U12. `documents.last_indexed` from `mark_indexed` is in seconds**
- **U13. `documents.synth_at` from `stamp_watermark` is in seconds**
- **U14. `ingest_runs.run_at` default is in seconds**
- **U15. Schema defaults: `curated_entities.created_at` falls back to seconds when
  the INSERT omits it** (uses raw SQL to bypass the app's writer and check the
  SQLite default still fires)

Each test uses a single helper:
```rust
fn assert_ms(ts: i64) { assert!(ts >= 1_000_000_000_000, "expected ms, got {ts}"); }
fn assert_sec(ts: i64) { assert!(ts < 1_000_000_000_000, "expected seconds, got {ts}"); }
```

`1_000_000_000_000` ms = 2001-09-09. Any timestamp after that distinguishes units
unambiguously. (This is the same heuristic the migration uses.)

The `timestamp_units.rs` test file lives at the crate top level so it can
exercise both `src-tauri/src/db/*` and `src-tauri/src/lib.rs` (heal functions
live in the latter). Cargo's `tests/` directory treats each file as a separate
integration-test binary, so we get full `pub(crate)` access via the existing
`pub(crate)` visibility on the helpers — no API surface changes needed.

## Test plan (executed by subagent before handoff)

- `cargo test -p tauri-app-lib --lib --quiet` — all unit tests pass, including
  D1–D5 and E1–E5.
- `cargo test -p tauri-app-lib --test timestamp_units --quiet` — all U1–U15 pass.
- `cargo test -p tauri-app-lib --quiet` — no regressions in any other test.
- On the live DB at `~/.brain-equational-wiki/brain.db`, run a smoke command
  the subagent invents: open a *copy*, apply MIGRATION_V12, count rows
  with `deleted_at < 1_000_000_000_000`. Expected: 0. Subagent reports the count
  but does NOT touch the live DB.

## Risks

- **The defensive "parse error → true" policy could mask a real future bug** where
  a new producer shape is silently treated as grounded. Mitigation: the
  `source_ref_is_still_grounded` helper logs a `tracing::warn!` on parse error
  so the operator sees it (and the U-test asserts no warning is emitted for
  the locked-in sentinel).
- **The 2 seconds-valued damaged rows on the live DB** were probably killed by
  `archive_fact` (which writes ms), not the heal path. MIGRATION_V12 fixes
  them anyway, but the implementer should NOT run the migration on the live
  DB without Kurt's go-ahead — include the recovery SQL in the PR description
  for review before the implementer runs it.
- **`heal_invalid_sources` writing ms now means its pruned (now ms-valued) rows
  will be reaped at a different cadence** until the migration runs on the live
  DB. The migration is part of the PR; the timing risk is the gap between
  merge and first DB open, which auto-applies V12.
- **The contract matrix is for *current* writers; future schema migrations might
  add new timestamped columns.** The new `timestamp_units.rs` test file is a
  forcing function for adding a U-test when that happens — but enforce this
  with a code review note in the PR, not code.
- **One existing test in `lib.rs:3449` (`heal_soft_deletes_missing_librarian_inferred_entries_only`)**
  will be removed or refactored. The replacement is E1 (alive JSON → no
  delete) and E2 (dead JSON → delete). The old test's behavior is preserved
  by the new E2 plus a no-op version of E1.

## Effort estimate

~1.5 dev days including:
- source_ref_is_still_grounded helper + 5 unit tests (4h)
- heal_lost_librarian_inferred + heal_invalid_sources fixes + 5 unit tests (3h)
- MIGRATION_V12 + idempotency test (1h)
- timestamp_units.rs with 15 unit tests (3h)
- PR review prep (1h)

## Verification (implementer must verify, not just claim)

- `cargo fmt --all` clean
- `cargo clippy -p tauri-app-lib --all-targets -- -D warnings` clean
- All test commands listed above return exit 0
- Subagent reports a successful run of the live-DB smoke (count of seconds-valued
  `deleted_at` rows after migration applied to a *copy* = 0)
- Subagent's final report includes the test output verbatim, not a summary

## Out of scope (filed as future work, not in this PR)

- Refactor `curated_entities` timestamps to ms (would touch bundle format, MCP wire
  format, all 23 reader/writer call sites in `entities.rs` and `bundle_apply.rs`).
- Re-architect the heal scheduler to be opt-in (separate from the watcher).
- Add a `source_ref_kind` column to `llm_wiki_entries` so future producers can
  declare their shape explicitly (would let `source_ref_is_still_grounded`
  become a simple lookup).
- Replace `tracing::warn!` on parse error with a structured schema-violation
  metric (depends on observability backend decision).
