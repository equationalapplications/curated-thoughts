# Issue #271 Two-Stage Retrieval Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add opt-in two-stage retrieval to the wisdom gate — stage-1 closed-set chunk search over fact-bearing documents mapped back to curated facts — behind the Option A design (per-doc `documents.embed_key` + query-time coverage check; NO pass/grant machinery), plus the destructive-pass safety fixes (§8) and the paired-live calibration harness (§7/§9 step 3).

**Architecture:** One PR, four build steps mirroring spec §9: (1) V28 DDL + the read path (stage-1 search, shared eligibility predicate, decision rules, v1-merge, coverage check, labels/audit, `--two-stage` flag); (2) safety + bootstrap (diff-swap transaction, funnel stamp + four-row decision table, cross-process deletion breaker, `librarian_evidence_seen` marker, `ct reindex verify-scratch`, `ct reindex repair-embed-key --stale`); (3) calibration harness (paired live driver + rule/floor picker + flip-to-default gated on the acceptance letter) — the harness SHIPS, it does not run calibration; (4) provenance backfill only if the audit shows hop gaps (expected: unnecessary — no task unless evidence appears).

**Tech Stack:** Rust (tauri_app_lib crate at `src-tauri/`, tools crate at `tools/`), rusqlite/SQLite (hand-rolled versioned migrations, `llm_wiki_meta` key-value table), clap v4 CLI (`tools/src/bin/ct.rs`), serde/serde_json, existing embedder (`EmbedProfile`, OpenAI-compatible HTTP + stub), existing `calibrate_wisdom_gate` bin.

**Spec:** `docs/superpowers/specs/2026-10-09-issue271-two-stage-retrieval-design.md` (rev 34, Approved). This plan argues from the spec; executors MUST read the spec sections cited per task. Verified codebase facts live in `/home/kv-thinkpad-t420-ubuntu/.hermes/cache/scratch/ct-271-impl-2026-10-15/survey-271/findings.md` (file:line refs below are verified against the branch as of `bc83d06`).

## Global Constraints (verbatim-critical — every task implicitly includes these)

- **Option A = NOTHING TO DELETE (plan-level fact):** `ct_reindex_pass_docs` never existed in code at `bc83d06`. There are no pass-docs tables, no grants, no refusal-record table, no `IngestOutcome` enum, no pass-aware sweep predicates, no frontend pass-progress fields to remove or build. Any task step implying them is a spec-reading error. (Spec §9 step 2 list, "Out of scope", Rulings carried.)
- **V28 ordering:** the new columns are added by **versioned migration V28** which runs **AFTER every versioned migration** (the V15 rebuild of `documents` copies an explicit column list at `src-tauri/src/db/connection.rs:300-303`; columns added before it would be dropped by the rebuild). V28's stamp is **gated on V22 having stamped**, like V23–V27.
- **V24-pattern mechanics (exactly `apply_v24_temporal_columns`, `src-tauri/src/db/connection.rs:881-950`):** unlocked column-existence pre-check via `crate::db::ddl_compat::existing_columns` (`src-tauri/src/db/ddl_compat.rs:128`); if missing → `BEGIN IMMEDIATE`, **re-inspect UNDER the lock**, ALTER, one-shot backfills, stamp; commit; `ROLLBACK` on error. Unlocked steady state never takes the write lock.
- **One-shot backfills INSIDE the ALTER call only** (V27 precedent `connection.rs:973-984`): any backfill statement executes ONLY in the migration call that adds its column, inside the same transaction. Re-running a backfill on every open is FORBIDDEN.
- **`wisdom_gate_decision` STDERR line is format-pinned:** the existing 5-field stderr line is byte-identical under both paths. The new `wisdom_two_stage_audit` stderr line is SEPARATE from it (§5).
- **Rust builds are SERIAL:** never run two `cargo` invocations concurrently in this repo (target-dir lock contention corrupts timing-sensitive tests). One build/test at a time.
- **Tests:** `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server -- --test-threads=1` (and the `tools` manifest for CLI tests). `--test-threads=1` always. Any `#[ignore]` test added by a task needs an explicit CI workflow step added in the same task.
- **Chunk-embedding model currency is PER-DOC (spec §6):** `documents.embed_key TEXT NULL` = `gate_model_key(profile, stub)` (`src-tauri/src/wisdom_match.rs:107-124`), the RAW key with NO scheme suffix, on BOTH write and read side. The `embeddings` table stays UNTAGGED (no scheme/model column is added anywhere in this PR). Stub-backed fixtures write/read the stub key consistently (`stub:constant8` shape per spec §6 — always compute it via `gate_model_key`, never hardcode).
- **Git:** regular merge commits only, NO squash. Conventional commits (`feat:`, `test:`, `fix:`, `chore:`). Branch `spec/issue271-two-stage-retrieval-impl`.
- **Pinned constants:** coverage threshold 100%; two-stage floor key shape `embed_scheme::floor_key_for(model, scheme) + ":two-stage:" + rule` with rule ∈ `restricted-cosine` | `chunk-hop` (§3); `TWO_STAGE_K: &[(&str, usize)]` beside `WISDOM_GATE_FLOORS`, default 8, k is NOT a floor key; audit line = exactly SEVEN fields in order `rule open top v1merged floor k stale_embed_key` (§6); breaker threshold `max(⌈0.05·L⌉, 10)` for all writers; bootstrap sample match tolerance cosine ≥ 1−1e−3; `MAX_EXCLUDES = 1024` (`wisdom_match.rs:39`).
- **Never-dark invariant:** every fallback (missing two-stage floor, coverage < 100%, schema skew) is v1's ONE rule (§4) — v1 fallback can open while a v1 floor exists; both floors missing = v1 uncalibrated semantics; the gate NEVER errors on schema skew of the probed columns.

---

## File Structure (what gets created/modified, by responsibility)

| File | Responsibility |
|---|---|
| `src-tauri/src/db/connection.rs` | `apply_v28_two_stage_columns` (V24 pattern) + call site in `migrate()` + guard-test update (:3377) + pre-V15 fixture test |
| `src-tauri/src/wisdom_match.rs` | shared eligibility predicate refactor (`gated_entries` :317), floor-key doc comment, `TWO_STAGE_K`, `wisdom_match_two_stage` entry point |
| `src-tauri/src/two_stage.rs` (NEW) | stage-1 candidate SQL, complement anti-join, coverage check with probes, chunk-score search + result-level dedupe, audit-line formatting |
| `tools/src/queries.rs` | `wisdom_match_cmd` :768 gains `two_stage` flag; embed-skip both-keys condition :791 |
| `tools/src/bin/ct.rs` | `--two-stage`/`--single-stage` on `WisdomCmd::Match` :354; new `Reindex` subcommand beside `Heal` :102 (enum `Cmd` :14); `Heal` gains `--reset-breaker`/`--allow-bulk` |
| `src-tauri/src/db/queries.rs` | `mark_document_indexed` :91 gains `indexed_hash` param |
| `src-tauri/src/pipeline/mod.rs` | diff-swap transaction in `ingest_file_virtual` :644 + four-row decision table + `IngestRefused` + refusal reset |
| `src-tauri/src/db/evidence_regrade.rs` | breaker pre-delete check :108-146, IMMEDIATE purge txn :134, id-set recount |
| `src-tauri/src/db/heal.rs` / `src-tauri/src/lib.rs` | breaker on `heal_invalid_sources_conn` :46 and `heal_lost_librarian_inferred` :2403 (IMMEDIATE + conditional UPDATE fix) |
| `src-tauri/src/chunker/mod.rs` | `CHUNKER_FINGERPRINT` constant |
| `tools/src/bin/bulk_reindex.rs` | `IngestRefused` downcast → SKIP (replaces abort :129-130) |
| `tools/src/bin/calibrate_wisdom_gate.rs` + new paired driver | paired live calibration harness + picker + flip-to-default |
| `src-tauri/tests/wisdom_gate_bench.rs` | `replay_two_stage` + per-key freeze-dir coverage assertion |
| `scripts/` | verify-scratch-before-calibration runner (step 3a ordering) |

---

# Task Group 1 — Read path + DDL (spec §9 step 1)

## Task 1: V28 migration — `documents.embed_key` + `documents.last_indexed_hash`

**Files:**
- Modify: `src-tauri/src/db/connection.rs` (call site ~:832 after `apply_v27_embed_scheme(conn)?;`; new fn beside `apply_v27_embed_scheme` :958)
- Test: `src-tauri/src/db/connection.rs` (tests module; guard test at :3360-3393; new pre-V15 fixture test)

**Interfaces:**
- Consumes: `crate::db::ddl_compat::existing_columns(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>>` (`src-tauri/src/db/ddl_compat.rs:128`).
- Produces: `fn apply_v28_two_stage_columns(conn: &Connection) -> Result<()>` (private, called from `migrate()`); columns `documents.embed_key TEXT` (NULL, NO backfill), `documents.last_indexed_hash TEXT` (one-shot backfill `WHERE status IN ('indexed','pending_reindex')`); stamped `schema_version` row 28 gated on V22. Later tasks call `ALTER`-free reads of these columns.

- [x] **Step 1: Write the failing tests**

Update the pinned-version guard test (the test asserting `version, 27` at `connection.rs:3372-3379`):

```rust
let version: i64 = conn
    .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
    .unwrap();
assert_eq!(
    version, 28,
    "V22 then V23 then V24 then V25 then V26 then V27 then V28 must be stamped \
     when the migration runs"
);
```

Add two tests in the same tests module:

```rust
#[test]
fn v28_adds_embed_key_and_last_indexed_hash_with_one_shot_backfill() {
    let conn = open_in_memory().unwrap();
    // open_in_memory ran migrate(): both columns exist, stamp 28 present.
    let cols = crate::db::ddl_compat::existing_columns(&conn, "documents").unwrap();
    assert!(cols.iter().any(|c| c == "embed_key"));
    assert!(cols.iter().any(|c| c == "last_indexed_hash"));
    let v: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, 28);

    // One-shot backfill semantics: an indexed row was backfilled from `hash`;
    // embed_key got NO migration backfill (NULL until a writer stamps it).
    conn.execute(
        "INSERT INTO documents (path, hash, status, tier) VALUES ('/v/a.md','h1','indexed','user_doc')",
        [],
    ).unwrap();
    // simulate a pre-upgrade pair by clearing the backfill on one row:
    conn.execute("UPDATE documents SET last_indexed_hash = NULL", []).unwrap();
    // Re-open is required to re-run migrate; use migrate_open_db directly:
    crate::db::connection::migrate_open_db(&conn, None).unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM documents WHERE last_indexed_hash IS NOT NULL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "backfill must be ONE-SHOT: a second migrate must NOT re-run it");
}

#[test]
fn v28_does_not_backfill_embed_key() {
    let conn = open_in_memory().unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM documents WHERE embed_key IS NOT NULL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "embed_key has NO migration backfill (writers only, spec §6)");
}
```

Add the pre-V15 fixture test (spec §9 step 1 requires asserting both columns exist after open on a pre-V15-shaped brain):

```rust
#[test]
fn pre_v15_fixture_gains_v28_columns_on_open() {
    // Build a documents table WITHOUT the V15-era columns the rebuild copies,
    // stamp nothing, then let migrate() run the full chain over it.
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE documents (id INTEGER PRIMARY KEY AUTOINCREMENT, path TEXT UNIQUE NOT NULL,
         hash TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'pending', tier TEXT NOT NULL DEFAULT 'user_doc');
         INSERT INTO documents (path, hash, status) VALUES ('/v/old.md','h-old','indexed');",
    )
    .unwrap();
    crate::db::connection::migrate_open_db(&conn, None).unwrap();
    let cols = crate::db::ddl_compat::existing_columns(&conn, "documents").unwrap();
    assert!(cols.iter().any(|c| c == "embed_key"));
    assert!(cols.iter().any(|c| c == "last_indexed_hash"));
    // The pre-V15 row survived the rebuild chain and was backfilled (status='indexed').
    let lih: Option<String> = conn
        .query_row("SELECT last_indexed_hash FROM documents WHERE path='/v/old.md'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(lih.as_deref(), Some("h-old"));
}
```

(If the repo keeps pre-V15 fixtures as SQL dump files under `src-tauri/tests/fixtures/`, reuse the existing V15-upgrade fixture instead of hand-building the table; keep the same assertions.)

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server -- --test-threads=1 v28`
Expected: FAIL — `embed_key` column does not exist / version asserted 28 but is 27.

- [x] **Step 3: Write the migration (V24 pattern, exact code)**

Add beside `apply_v27_embed_scheme` (`connection.rs:958`):

```rust
/// V28 body (spec 2026-10-09-issue271 §6/§8(c)/§9 step 1): add
/// `documents.embed_key TEXT` (model key whose vectors the doc's chunks carry;
/// NULL/stale ⇒ excluded from stage 1 — written by the §8(a) swap transaction
/// and ordinary ingests, NEVER by a migration backfill) and
/// `documents.last_indexed_hash TEXT` (hash of the bytes actually chunked and
/// embedded; written ONLY inside `mark_document_indexed`), with the ONE-SHOT
/// `last_indexed_hash` backfill. Runs AFTER every versioned migration (the V15
/// rebuild of `documents` copies an explicit column list and would drop the
/// columns otherwise). Stamp gated on V22 like V23–V27.
fn apply_v28_two_stage_columns(conn: &Connection) -> Result<()> {
    // Unlocked pre-check keeps the steady state (both columns present) from
    // taking the write lock on every open — the V24/V27 pattern.
    let existing = crate::db::ddl_compat::existing_columns(conn, "documents")?;
    let missing_embed_key = !existing.iter().any(|c| c == "embed_key");
    let missing_lih = !existing.iter().any(|c| c == "last_indexed_hash");
    if missing_embed_key || missing_lih {
        conn.execute_batch("BEGIN IMMEDIATE;")?;
        let applied = (|| -> Result<()> {
            // Re-inspect UNDER the write lock (concurrent desktop/`--mcp` open).
            let existing = crate::db::ddl_compat::existing_columns(conn, "documents")?;
            if !existing.iter().any(|c| c == "embed_key") {
                conn.execute("ALTER TABLE documents ADD COLUMN embed_key TEXT", [])?;
            }
            if !existing.iter().any(|c| c == "last_indexed_hash") {
                conn.execute("ALTER TABLE documents ADD COLUMN last_indexed_hash TEXT", [])?;
                // ONE-SHOT backfill, inside the ALTER call only (V27 precedent,
                // connection.rs:973-984): `pending` rows may already carry
                // queue.rs's pre-written NEW hash (spec §8(c) B1); pending_reindex
                // rows were staged from indexed with unchanged bytes ⇒ backfilled.
                conn.execute(
                    "UPDATE documents SET last_indexed_hash = hash
                      WHERE status IN ('indexed','pending_reindex')",
                    [],
                )?;
            }
            Ok(())
        })();
        if let Err(e) = applied.and_then(|()| Ok(conn.execute_batch("COMMIT;")?)) {
            let _ = conn.execute_batch("ROLLBACK;");
            return Err(e);
        }
    }
    // Stamp last, gated on V22 (a rootless open must not mask a deferred V22).
    let stamped_now: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;
    if stamped_now >= 22 {
        conn.execute(
            "INSERT OR IGNORE INTO schema_version (version) VALUES (28)",
            [],
        )?;
    }
    Ok(())
}
```

Call it in `migrate()` immediately after `apply_v27_embed_scheme(conn)?;` (`connection.rs:832`):

```rust
apply_v28_two_stage_columns(conn)?;
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server -- --test-threads=1 v28` and then the full connection test module (same command, filter `db::connection`).
Expected: PASS. Also run the full suite once (Global Constraints command) — no other test may regress.

- [x] **Step 5: Commit**

```bash
git add src-tauri/src/db/connection.rs
git commit -m "feat(db): V28 adds documents.embed_key + last_indexed_hash (one-shot backfill, V22-gated stamp)"
```

## Task 2: Shared eligibility predicate (refactor `gated_entries` onto it) + >1024-excludes parity guard

**Files:**
- Modify: `src-tauri/src/wisdom_match.rs` (gated_entries at :317, `temporal_columns` :176-186, `CandidateRow` :308)
- Test: `src-tauri/src/wisdom_match.rs` (tests module)

**Interfaces:**
- Consumes: `Temporal { has_superseded_by: bool, has_valid_to: bool, scheme: Option<Scheme> }` (`wisdom_match.rs:162-171`); `crate::embed_scheme::scheme_filter_sql(Option<Scheme>) -> String` (`src-tauri/src/embed_scheme.rs:212`); `existing_columns` (Task 1 Interfaces).
- Produces (used by Task 3's stage-1 SQL and Task 5's decision rules):
  - `pub(crate) fn entry_eligibility_where(t: &Temporal) -> String` — ONE SQL WHERE fragment implementing the live-gate-eligible test (deleted_at IS NULL, NULL-only title/body test, supersession/validity per `Temporal` columns, `{scheme_filter}` interpolated). The enumeration is EXHAUSTIVE and MUST include the V27 read-scheme filter: stage-1 SQL consumes `scheme_filter_sql(temporal.scheme)` (`src-tauri/src/embed_scheme.rs:212`) as part of this predicate — a stage-1 query without it is a spec §3.2 violation.
  - `pub(crate) fn eligible_entry_ids(conn: &Connection, t: &Temporal, now_ms: i64) -> Result<Vec<String>>` — the same predicate as a standalone query, so Rust-side consumers share it.
  - Behavioral invariant: `gated_entries` (wisdom_match.rs:317) and stage-1's candidate/complement queries use THIS fragment — they can never disagree about which facts are eligible (§3.2).

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn eligibility_predicate_matches_gated_entries_exactly() {
    let conn = open_in_memory().unwrap();
    // seed(): live eligible, soft-deleted, NULL-title, superseded, expired.
    seed(&conn, "live", "ent", &at(0.9));
    conn.execute("UPDATE llm_wiki_entries SET deleted_at = 1 WHERE id = 'live2'", []).ok();
    seed_deleted(&conn, "dead");
    seed_null_title(&conn, "notitle");
    seed_superseded(&conn, "old", "new");
    seed_expired(&conn, "expired");

    let temporal = temporal_columns(&conn, None).unwrap();
    let ids = eligible_entry_ids(&conn, &temporal, NOW_MS).unwrap();
    let gated = gated_entries(
        &conn, &vec![0.5f32; 8], 0.0, 10, &HashSet::new(), NOW_MS, &temporal,
    ).unwrap();
    let gated_ids: HashSet<String> = gated.iter().map(|e| e.0.clone()).collect();
    let pred_ids: HashSet<String> = ids.into_iter().collect();
    assert_eq!(gated_ids, pred_ids, "one predicate, two consumers");
}

#[test]
fn stage1_predicate_parity_above_1024_excludes() {
    // The per-call skip set lives in Rust on BOTH paths (spec §3.1 r30-M2):
    // parity guard — with MORE than MAX_EXCLUDES excludes, both the v1 path
    // and the stage-1 complement computation skip exactly the same ids.
    let conn = open_in_memory().unwrap();
    seed(&conn, "a", "ent", &at(0.9));
    seed(&conn, "b", "ent", &at(0.8));
    let excludes: Vec<String> = (0..1030).map(|i| format!("x{i}")).chain(["a".into()]).collect();
    let temporal = temporal_columns(&conn, None).unwrap();
    let eligible = eligible_entry_ids(&conn, &temporal, NOW_MS).unwrap();
    let skip: HashSet<String> = excludes.iter().cloned().collect();
    let after_skip: Vec<_> = eligible.into_iter().filter(|id| !skip.contains(id)).collect();
    assert_eq!(after_skip, vec!["b".to_string()]);
    // v1 path with the same oversize skip set (gated_entries takes &HashSet):
    let gated = gated_entries(&conn, &vec![0.5f32; 8], 0.0, 10, &skip, NOW_MS, &temporal).unwrap();
    assert_eq!(gated.iter().map(|e| e.0.clone()).collect::<Vec<_>>(), vec!["b".to_string()]);
}
```

(Helper seeds `seed_deleted`/`seed_null_title`/`seed_superseded`/`seed_expired` follow the existing `seed` helper's INSERT shape in the tests module; set `deleted_at`, NULL `title`, `superseded_by`/`superseded_at`, `valid_to` respectively.)

- [x] **Step 2: Run to verify FAIL** — `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server -- --test-threads=1 eligibility_predicate` → FAIL (functions don't exist).

- [x] **Step 3: Minimal implementation**

Extract the WHERE-fragment builder from `gated_entries`'s SELECT (the predicate currently inlined at `wisdom_match.rs:317+`) into:

```rust
/// THE shared eligibility predicate (spec §3.2): ONE WHERE fragment consumed
/// by `gated_entries` AND the stage-1 candidate/complement queries in
/// `two_stage`, so the two stages can never disagree about eligibility.
pub(crate) fn entry_eligibility_where(t: &Temporal) -> String {
    let scheme_filter = crate::embed_scheme::scheme_filter_sql(t.scheme);
    let mut sql = format!(
        "e.deleted_at IS NULL \
         AND e.title IS NOT NULL AND e.body IS NOT NULL{scheme_filter}"
    );
    if t.has_superseded_by {
        sql.push_str(" AND (e.superseded_by IS NULL OR e.superseded_at <= :now)");
    }
    if t.has_valid_to {
        sql.push_str(" AND (e.valid_to IS NULL OR e.valid_to > :now)");
    }
    sql
}

pub(crate) fn eligible_entry_ids(conn: &Connection, t: &Temporal, now_ms: i64) -> Result<Vec<String>> {
    let where_sql = entry_eligibility_where(t);
    let sql = format!(
        "SELECT e.id FROM llm_wiki_entries e WHERE {where_sql}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::named_params! { ":now": now_ms }, |r| r.get::<_, String>(0))?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}
```

Rewrite `gated_entries`'s SELECT to interpolate `entry_eligibility_where(temporal)` in place of its inlined predicate. No behavior change otherwise (the `raw < floor` drop at :379-381 stays in Task 2 — rule (ii)'s floor-bypass parameter lands in Task 5).

- [x] **Step 4: Run to verify PASS** — full `wisdom_match` test module + the two new tests.
- [x] **Step 5: Commit** — `git commit -m "refactor(wisdom): extract shared entry eligibility predicate (stage-1 parity, >1024 excludes guard)"`

## Task 3: `two_stage` module — candidate set, complement, stage-1 search

**Files:**
- Create: `src-tauri/src/two_stage.rs`; register `pub mod two_stage;` in `src-tauri/src/lib.rs`
- Test: `src-tauri/src/two_stage.rs` (tests module, stub-embedder fixtures)

**Interfaces:**
- Consumes: `entry_eligibility_where`/`eligible_entry_ids` + `Temporal` (Task 2); `evidence_has_live_chunk`'s hash resolution shape (`src-tauri/src/db/commit.rs:666-690`); `curated_proposal_sources (proposal_id, doc_id, role)` DDL (`src-tauri/src/db/okf_ddl.rs:197-202`); `librarian_evidence` DDL (`src-tauri/src/db/schema.rs:392-403`); `existing_columns` (Task 1); `crate::search::cosine_similarity` (`src-tauri/src/search/mod.rs`).
- Produces (Task 4, 5, 6, 14, 15, 16 consume these):

```rust
/// One stage-1 candidate population. `doc_ids` = evidence-hash hop ∪ proposal
/// chain, hash-hop deduped at DOC level is FORBIDDEN (r29-M2: membership is by
/// doc_id; dedupe applies ONLY at result level) — this is the raw UNION.
/// `complement_entry_ids(rule)` = gate-eligible entries NOT reachable by that
/// rule's edge (structural, computed by anti-join — r27-M1/r29-M1).
pub struct CandidateSet { pub doc_ids: Vec<i64> }

/// Probes then computes. `Ok(None)` = probed column/table missing (pre-V28 /
/// pre-V18 brain) ⇒ caller MUST take the §4 v1 fallback. (spec §6 r28-M3/r30-m2)
pub fn candidate_set(conn: &Connection, now_ms: i64, scheme: Option<crate::embed_scheme::Scheme>)
    -> Result<Option<CandidateSet>>;

/// Rule-relative complement (§3 v1-merge): eligible entry ids NOT reachable.
/// rule="restricted-cosine": reachable iff carried by a candidate doc with
///   >=1 stage-1-scorable chunk (>=1 chunk with an embeddings row).
/// rule="chunk-hop": reachable iff carried by a candidate doc with >=1 MEMBER
///   chunk (hash-hop-hit) that is stage-1-scorable.
pub fn complement_entry_ids(conn: &Connection, cands: &CandidateSet, rule: &str,
    now_ms: i64, scheme: Option<crate::embed_scheme::Scheme>) -> Result<Vec<String>>;

/// One scored stage-1 chunk (result-level dedupe ALREADY applied: duplicate
/// identical vectors collapse — same text ⇒ same vector ⇒ keep one; NO
/// row-level doc dedupe, r29-M2).
pub struct Stage1Hit { pub chunk_id: i64, pub doc_id: i64, pub score: f32 }
/// Top-k by cosine over candidate-doc chunks WITH embeddings rows, raw query
/// vector (no scheme prefix — §3.1).
pub fn stage1_search(conn: &Connection, query_vec: &[f32], cands: &CandidateSet, k: usize)
    -> Result<Vec<Stage1Hit>>;

/// Chunk-level membership for rule (ii): chunk ids of candidate docs whose
/// content_hash appears in a live librarian_evidence evidence blob.
pub fn member_chunk_ids(conn: &Connection, cands: &CandidateSet) -> Result<Vec<i64>>;

/// §6 coverage over candidate docs with >=1 stage-1-scorable chunk, BEFORE the
/// per-call exclude/correction skip set (never call-dependent). Returns
/// (candidate_doc_count, covered_count); `Ok(None)` = probe failed ⇒ v1 fallback.
pub fn coverage(conn: &Connection, profile_key: &str) -> Result<Option<(i64, i64)>>;
```

- [x] **Step 1: Write the failing tests** (fixture seeds documents + chunks + embeddings + librarian_evidence + curated_proposal_sources rows directly with SQL; embeddings vectors chosen orthonormal so cosine is exact)

```rust
fn fixture() -> Connection {
    let conn = open_in_memory().unwrap();
    // docs 1,2 candidate (1 via evidence hash hop, 2 via proposal chain);
    // doc 3 NOT candidate; doc 4 candidate but ZERO chunks (r26-M2).
    for (id, key) in [(1, "stub:constant8"), (2, "stub:constant8"), (3, "old-model"), (4, "")] {
        conn.execute(
            "INSERT INTO documents (path, hash, status, tier, embed_key)
             VALUES (?1, ?1, 'indexed', 'user_doc', ?2)",
            rusqlite::params![format!("/v/d{id}.md"), if key.is_empty() { None } else { Some(key) }],
        ).unwrap();
    }
    // chunks: doc1 c1,e1; doc2 c2,e1; doc3 c3,e1; e-block on doc4 = none
    for (cid, did) in [(1, 1), (2, 2), (3, 3)] {
        conn.execute("INSERT INTO chunks (doc_id, chunk_text, position, strategy) VALUES (?1,'t',0,'prose')", [did]).unwrap();
        // give chunk cid an embeddings row except c3 (doc3 stays scorable for negative control)
        if cid != 3 {
            conn.execute("INSERT INTO embeddings (chunk_id, vector) VALUES (?1, ?2)",
                rusqlite::params![cid, crate::search::f32_to_bytes(&unit_vec(cid))]).unwrap();
        }
    }
    conn.execute_batch("
        INSERT INTO curated_proposals (id, status) VALUES ('p1','approved');
        INSERT INTO curated_proposal_sources (proposal_id, doc_id, role) VALUES ('p1',2,'evidence');
        INSERT INTO llm_wiki_entries (id, entity_id, title, body, source_type, source_ref) VALUES
          ('f1','e1','T1','B1','librarian_inferred','x'),
          ('f2','e2','T2','B2','user_stated',NULL),
          ('f3','e3','T3','B3','librarian_inferred','x');
        INSERT INTO librarian_evidence (entry_id, proposal_id, evidence_json, created_at) VALUES
          ('f1','p1','{\"evidence\":[{\"content_hash\":\"h-c1\"}]}',0),
          ('f3','p1','{\"evidence\":[{\"content_hash\":\"h-c9-gone\"}]}',0);
        UPDATE chunks SET content_hash='h-c1' WHERE id=1;
    ").unwrap();
    conn
}
```

(Adjust INSERT column lists to the actual table schemas on the branch — the survey's `schema.rs:392-403` and `okf_ddl.rs:197-202` pin `librarian_evidence` and `curated_proposal_sources`; `curated_proposals` per its existing DDL.)

```rust
#[test]
fn candidate_set_is_union_of_both_edges_no_doc_dedupe() {
    let conn = fixture();
    let cs = candidate_set(&conn, NOW_MS, None).unwrap().unwrap();
    // doc1 (evidence hash hop) ∪ doc2 (proposal chain) ∪ doc4 (proposal/none — whatever edges reach)
    assert!(cs.doc_ids.contains(&1) && cs.doc_ids.contains(&2));
    assert!(!cs.doc_ids.contains(&3), "non-candidate doc excluded");
}

#[test]
fn stage1_search_dedupes_identical_vectors_at_result_level_only() {
    let conn = fixture();
    // c1 and c2 carry IDENTICAL vectors ⇒ one result even though two docs.
    let cs = candidate_set(&conn, NOW_MS, None).unwrap().unwrap();
    let hits = stage1_search(&conn, &unit_vec(1), &cs, 8).unwrap();
    let scores: Vec<f32> = hits.iter().map(|h| h.score).collect();
    let dupes = scores.iter().filter(|s| (**s - 1.0).abs() < 1e-6).count();
    assert_eq!(dupes, 1, "identical vectors collapse in top-k (r29-M2)");
}

#[test]
fn complement_is_structural_per_rule() {
    let conn = fixture();
    let cs = candidate_set(&conn, NOW_MS, None).unwrap().unwrap();
    let comp_i = complement_entry_ids(&conn, &cs, "restricted-cosine", NOW_MS, None).unwrap();
    // f2 (user_stated, no edge) + f3 (evidence hash h-c9-gone resolves to
    // nothing live AND its proposal edge p1→doc2 IS a scorable candidate ⇒
    // f3 is rule-(i)-REACHABLE via the proposal chain, but rule-(ii)-dark
    // unless a member chunk exists).
    assert!(comp_i.contains(&"f2".to_string()));
    assert!(!comp_i.contains(&"f3".to_string()), "proposal edge ⇒ rule-(i) reachable");
    let comp_ii = complement_entry_ids(&conn, &cs, "chunk-hop", NOW_MS, None).unwrap();
    assert!(comp_ii.contains(&"f2".to_string()));
    // doc2 (proposal-chain-only candidate, zero member chunks) ⇒ its facts are
    // outside rule (ii)'s reach ⇒ complement (r29-M1).
    // f1 IS reachable under (ii): chunk 1 is a member chunk (hash h-c1 in f1's evidence).
    assert!(!comp_ii.contains(&"f1".to_string()));
}

#[test]
fn coverage_counts_only_scorable_candidate_docs_and_is_call_independent() {
    let conn = fixture();
    let (n, cov) = coverage(&conn, "stub:constant8").unwrap().unwrap();
    // population: candidate docs with >=1 embeddings-row chunk = docs 1,2.
    assert_eq!((n, cov), (2, 2));
    assert_eq!(coverage(&conn, "old-model").unwrap().unwrap(), (2, 0));
}

#[test]
fn probes_return_none_on_schema_skew() {
    let conn = Connection::open_in_memory().unwrap();
    // bare DB: no documents.embed_key column, no librarian_evidence table.
    conn.execute_batch("CREATE TABLE documents (id INTEGER PRIMARY KEY, path TEXT);").unwrap();
    assert!(candidate_set(&conn, NOW_MS, None).unwrap().is_none());
    assert!(coverage(&conn, "k").unwrap().is_none());
}
```

- [x] **Step 2: Run to verify FAIL** — `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server -- --test-threads=1 two_stage` → FAIL (module missing).

- [x] **Step 3: Minimal implementation (exact SQL)**

```rust
//! Stage-1 closed-set chunk search for the wisdom gate (spec §3.1, §4 GUARD, §6).
use anyhow::Result;
use rusqlite::Connection;

use crate::db::ddl_compat::existing_columns;
use crate::wisdom_match::{entry_eligibility_where, Temporal};

/// BOTH probes pinned (spec §6 r28-M3 + r30-m2 + r29-m5): `documents` columns
/// (embed_key AND last_indexed_hash) plus table existence via sqlite_master.
/// Any miss ⇒ Ok(None) ⇒ caller takes the §4 v1 fallback. The gate never
/// errors on schema skew of the probed columns; it degrades to v1. Stage-1
/// SQL runs ONLY after the hop tables are confirmed (never-darker than V18).
pub fn two_stage_schema_ready(conn: &Connection) -> Result<bool> {
    if !table_exists(conn, "librarian_evidence")? { return Ok(false); }
    if !table_exists(conn, "curated_proposal_sources")? { return Ok(false); }
    if !table_exists(conn, "chunks")? { return Ok(false); }
    let cols = existing_columns(conn, "documents")?;
    Ok(cols.iter().any(|c| c == "embed_key") && cols.iter().any(|c| c == "last_indexed_hash"))
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1", [name], |r| r.get(0))?;
    Ok(n > 0)
}

/// Evidence-hash hop ∪ proposal chain (r28-M1 — the REAL edges; there is no
/// `source_hash` column on curated_proposal_sources and `documents.hash` is
/// NOT a membership predicate). A dangling proposal leg JOINS TO NOTHING and
/// lands in the complement — never lean on CASCADE (CT connections never set
/// PRAGMA foreign_keys; librarian_evidence.proposal_id has no FK).
const CANDIDATE_DOCS_SQL: &str = "
SELECT DISTINCT doc_id FROM (
  SELECT DISTINCT c.doc_id AS doc_id
    FROM librarian_evidence le
    JOIN llm_wiki_entries e ON e.id = le.entry_id
    JOIN json_each(le.evidence_json, '$.evidence') je
    JOIN chunks c ON c.content_hash = json_extract(je.value, '$.content_hash')
    WHERE {elig}
  UNION
  SELECT DISTINCT ps.doc_id AS doc_id
    FROM librarian_evidence le
    JOIN llm_wiki_entries e ON e.id = le.entry_id
    JOIN curated_proposal_sources ps ON ps.proposal_id = le.proposal_id
    WHERE {elig}
)";
```

`candidate_set` builds `elig` via `entry_eligibility_where(&Temporal)` (alias `e` = `llm_wiki_entries`), returns `None` when `!two_stage_schema_ready`, else `Ok(Some(CandidateSet { doc_ids }))`.

Member chunks + complement per rule:

```sql
-- member chunks (rule ii): candidate-doc chunks whose hash is referenced by live evidence
SELECT DISTINCT c.id FROM chunks c
WHERE c.doc_id IN (SELECT doc_id FROM (candidate-docs))
  AND c.id IN (SELECT emb.chunk_id FROM embeddings emb)          -- stage-1-scorable
  AND c.content_hash IN (
        SELECT json_extract(je.value, '$.content_hash')
        FROM librarian_evidence le
        JOIN llm_wiki_entries e2 ON e2.id = le.entry_id AND e2.deleted_at IS NULL
        JOIN json_each(le.evidence_json, '$.evidence') je
        WHERE json_extract(je.value, '$.content_hash') IS NOT NULL);

-- rule-(i) reachable entries: entries with an edge into a candidate doc that has >=1 scorable chunk
SELECT DISTINCT e.id FROM llm_wiki_entries e
WHERE {elig}
  AND e.id IN (
    (SELECT le.entry_id FROM librarian_evidence le
       JOIN json_each(le.evidence_json,'$.evidence') je
       JOIN chunks c ON c.content_hash = json_extract(je.value,'$.content_hash')
       JOIN embeddings emb ON emb.chunk_id = c.id
     WHERE c.doc_id IN (candidate-docs))
    UNION
    (SELECT le.entry_id FROM librarian_evidence le
       JOIN curated_proposal_sources ps ON ps.proposal_id = le.proposal_id
     WHERE ps.doc_id IN (candidate-docs)
       AND EXISTS (SELECT 1 FROM chunks c JOIN embeddings emb ON emb.chunk_id=c.id
                    WHERE c.doc_id = ps.doc_id)));
-- rule-(ii) reachable: same but the proposal leg ALSO requires the doc to have
-- >=1 MEMBER chunk (chunk-id-only evidence counts as COMPLEMENT — spec §3 class 5, r30-m4 scoping);
-- implement as the rule-(i) query minus the unqualified proposal leg, plus the member-chunk join.
```

`complement_entry_ids` = `eligible_entry_ids` MINUS the reachable set (anti-join in Rust or SQL `NOT IN` — both fine; the spec requires the complement be computed structurally, not by provenance).

Stage-1 search + result-level dedupe:

```rust
pub fn stage1_search(conn: &Connection, query_vec: &[f32], cands: &CandidateSet, k: usize)
    -> Result<Vec<Stage1Hit>> {
    let id_list = cands.doc_ids.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT c.id, c.doc_id, emb.vector FROM chunks c \
         JOIN embeddings emb ON emb.chunk_id = c.id \
         WHERE c.doc_id IN ({id_list})");
    let mut stmt = conn.prepare(&sql)?;
    let mut scored: Vec<Stage1Hit> = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, Vec<u8>>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|(chunk_id, doc_id, bytes)| {
            let v = crate::search::bytes_to_f32(&bytes);
            Stage1Hit { chunk_id, doc_id, score: crate::search::cosine_similarity(query_vec, &v) }
        })
        .collect();
    // r29-M2: dedupe at RESULT level only — duplicate identical VECTORS keep one.
    // (score equality on identical vectors; keep the lowest chunk_id for determinism)
    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap()
        .then(a.chunk_id.cmp(&b.chunk_id)));
    let mut out: Vec<Stage1Hit> = Vec::new();
    for h in scored {
        if out.last().map(|p: &Stage1Hit| (p.score - h.score).abs() < 1e-6).unwrap_or(false)
            && out.len() >= 1 && same_vector(conn, out.last().unwrap().chunk_id, h.chunk_id)? { continue; }
        out.push(h);
    }
    out.truncate(k);
    Ok(out)
}
```

(`same_vector` compares the two embeddings rows' bytes exactly — identical text ⇒ identical stored vector; a score tie without byte equality is a genuine near-duplicate and stays.)

Coverage (§6 SQL, verbatim shape):

```rust
pub fn coverage(conn: &Connection, profile_key: &str) -> Result<Option<(i64, i64)>> {
    if !two_stage_schema_ready(conn)? { return Ok(None); }
    // candidate set BEFORE the embed_key exclusion (r26-M2: population =
    // candidate docs with >=1 stage-1-scorable chunk, computed before the
    // embed_key predicate and before any per-call skip set).
    let cands = match candidate_set(conn, 0, None)? { Some(c) => c, None => return Ok(None) };
    if cands.doc_ids.is_empty() { return Ok(Some((0, 0))); }  // vacuously covered
    let id_list = cands.doc_ids.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT COUNT(*), COALESCE(SUM(CASE WHEN d.embed_key = ?1 THEN 1 ELSE 0 END), 0) \
           FROM documents d \
          WHERE d.id IN ({id_list}) \
            AND EXISTS (SELECT 1 FROM chunks c JOIN embeddings emb ON emb.chunk_id = c.id \
                         WHERE c.doc_id = d.id)");
    let (n, cov) = conn.query_row(sql, [profile_key], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
    })?;
    Ok(Some((n, cov)))
}
```

NOTE on `candidate_set(conn, 0, None)` inside `coverage`: pass the real `now_ms` through a parameter instead — coverage takes `now_ms: i64` too (update the signature and callers; `now_ms` feeds only the eligibility predicate).

- [x] **Step 4: Run to verify PASS** — `cargo test ... -- --test-threads=1 two_stage` then the full suite.
- [x] **Step 5: Commit** — `git commit -m "feat(two-stage): stage-1 candidate set (evidence-hash hop ∪ proposal chain), rule-relative complement, coverage check"`

## Task 4: Two-stage decision core in `wisdom_match.rs` — floors, rules, v1-merge, corrections parity

**Files:**
- Modify: `src-tauri/src/wisdom_match.rs` (floors table :53 + doc comment :50-52; new pub fns near `wisdom_match_impl`)
- Test: `src-tauri/src/wisdom_match.rs` (tests module)

**Interfaces:**
- Consumes: `two_stage::{candidate_set, complement_entry_ids, stage1_search, member_chunk_ids, coverage, two_stage_schema_ready}` (Task 3); `gate_model_key` (`wisdom_match.rs:107-124`); `floor_key_for` (`src-tauri/src/embed_scheme.rs:135`); `wisdom_match_with_floor` (existing, used by `calibrate_wisdom_gate.rs:29`); `gated_entries` with its `raw < floor` drop at :379-381; `WISDOM_GATE_FLOORS: &[(&str, f32)]` (:53).
- Produces (Task 5's CLI + Task 15/16 harness consume):

```rust
/// beside WISDOM_GATE_FLOORS (NOT a floor key — Opus r3 M3):
pub const TWO_STAGE_K: &[(&str, usize)] = &[("default", 8)];
pub fn two_stage_k(gate_key: &str) -> usize; // mirrors gate_floor's lookup, default 8

/// Rule + per-rule decision variable for the audit line (§6 r28-m3).
pub enum TwoStageRule { RestrictedCosine, ChunkHop }

pub struct TwoStageOutcome {
    pub rule: Option<TwoStageRule>,   // None = no two-stage rule opened (§6 m4)
    pub open: bool,                   // pinned: FINAL entries list non-empty (r30-m5)
    pub top: Option<f32>,             // the evaluated rule's OWN decision variable
    pub v1merged: usize,              // facts admitted by the v1-merge
    pub floor_key_used: String,
    pub k: usize,
    pub stale_embed_key: i64,         // candidate docs (>=1 chunk) with NULL/stale key
}

/// The whole two-stage evaluation. `exclude` already contains correction heads
/// ("one id, one list", wisdom_match.rs:293-296). Corrections are computed by
/// the CALLER unchanged and passed in (r29-m2: identical arrays under both paths).
pub fn two_stage_entries(
    conn: &Connection,
    query_vec_raw: &[f32],          // stage-1 embed (raw query text, no prefix)
    query_vec_schemed: &[f32],      // rule-(i)/merge vector: query_text_for_scheme embed
    scheme: crate::embed_scheme::Scheme,
    v1_floor: Option<f32>,          // gate_floor(current v1 key)
    two_stage_floor: Option<f32>,   // gate_floor(floor_key_for(key, scheme) + ":two-stage:" + rule)
    max: usize,
    exclude: &std::collections::HashSet<String>,
    corrections_absent_ids: &std::collections::HashSet<String>,
    now_ms: i64,
    profile_key: &str,
) -> Result<(Vec<crate::wisdom_match::WisdomItem>, TwoStageOutcome)>;
```

Floor-key selection during calibration uses composite keys passed to `gate_floor` directly (`floor_key_for(model, scheme) + ":two-stage:" + rule`), matching `replay_two_stage`'s later assertion `gate_floor(gate_key) == expected.floor` (`src-tauri/tests/wisdom_gate_bench.rs:107-111`).

**§4 rule (implement EXACTLY this matrix, ONE rule):**
1. two-stage floor MISSING + v1 floor present → v1 fallback (v1 embed, v1 floors, v1 label).
2. BOTH missing → uncalibrated: `gate = "uncalibrated"`, `entries = vec![]`, embed skipped (caller).
3. two-stage floor PRESENT + v1 floor MISSING + coverage < 100% → uncalibrated no-open (dark-adjacent BY DESIGN).
4. two-stage floor present (+ v1 floor any) + coverage = 100% → two-stage runs (rule = calibrated rule for this key).
5. Below-floor stage 1 → gate closed, audit line STILL emitted (`rule=i|ii open=0` — never `rule=none` on a two-stage close).

- [x] **Step 1: Write the failing tests** — fixtures reuse Task 3's `fixture()` via `pub(crate)` re-export or a tests-only helper module `#[cfg(test)] pub(crate) mod two_stage_test_support`. Core rows:

```rust
#[test]
fn missing_two_stage_floor_falls_back_to_v1() { /* v1 floor present, two-stage None ⇒ v1 entries, outcome.rule None */ }

#[test]
fn both_floors_missing_is_uncalibrated_no_embed() { /* outcome.rule None; caller asserts gate label */ }

#[test]
fn coverage_below_100_falls_back_with_stale_count() {
    // one stale doc in candidate set ⇒ v1 entries; outcome.stale_embed_key == 1
}

#[test]
fn fourth_cell_two_stage_floor_only_plus_low_coverage_is_uncalibrated() { /* no open, rule None */ }

#[test]
fn rule_i_opens_on_restricted_fact_cosine() { /* calibrated rule i; open=1; top=best restricted fact cosine */ }

#[test]
fn rule_ii_bypasses_the_fact_floor() {
    // member chunk clears chunk-hop floor; opened doc's facts have raw < v1 floor
    // ⇒ still injected (r25-M4): entries non-empty, ranked by weighted cosine
}

#[test]
fn rule_ii_uses_evidence_hash_edge_not_proposal_union() {
    // two-doc proposal (trigger doc B, evidence doc A); probe matches B ⇒
    // A's facts do NOT open under rule (ii) (r30-M3 fixture)
}

#[test]
fn v1_merge_keeps_source_less_brain_openable() {
    // ALL-user_stated brain: no candidate docs, v1 floor present ⇒ open via merge, v1merged >= 1
}

#[test]
fn zero_chunk_doc_facts_open_via_merge() { /* r27-M1 */ }

#[test]
fn corrections_identical_and_heads_excluded() { /* r29-m2 parity vs v1 call */ }

#[test]
fn audit_outcome_fields_pinned() { /* rule/open/top/v1merged/floor_key_used/k/stale semantics */ }
```

- [x] **Step 2: Run to verify FAIL.**
- [x] **Step 3: Implement.** Decision core sketch (fills the five rule rows + rule mechanics):

```rust
// Rule (i): score facts of candidate docs (union hop, doc-level) with the
// SCHEME-PREFIXED query vector; open iff best restricted fact cosine >= two_stage_floor.
// Fact vectors: llm_wiki_entries.embedding_blob via the SAME decode used by
// gated_entries (:375 skips dimension mismatch). Candidates ranked by the
// weighted-cosine path (tier weight + id tiebreak, :391-400).
//
// Rule (ii): top-1 MEMBER chunk (highest-scoring stage1 hit that is in
// member_chunk_ids) above the chunk-hop floor ⇒ open on ALL facts whose
// evidence content_hash resolves into that chunk's doc (EVIDENCE-HASH edge,
// r30-M3) with NO fact-level floor; bypass gated_entries' raw<floor drop by
// passing floor=None into the selection loop (r26 m2) — reuse its weighted
// sort + truncate-to-max. Chunk score is NEVER an entry score.
//
// v1-merge: complement_entry_ids(rule) scored via the v1 path with the
// SCHEME-PREFIXED vector (r28-m2), floor = v1 floor; bit-identical to
// wisdom_match_in_scheme's score for the same fact (test asserts parity).
// Merge into ONE ranked list (weighted cosine, truncate max).
//
// corrections: caller-computed; skip set = exclude ∪ correction heads, applied
// to rule (i), rule (ii) AND the merge identically.
```

Also update the `WISDOM_GATE_FLOORS` doc comment (:50-52) — a `chunk-hop` floor abstains on a chunk score, not raw cosine (Opus r5 MINOR-4).

- [x] **Step 4: Run to verify PASS** (module + full suite, `--test-threads=1`) — run 2026-10-15: `wisdom_match` 36/0 · `two_stage` 8/0 · FULL suite (`--features test-utils,mcp-server`) 1697 passed / 0 failed, exit 0 (fix round sa-0-8ab57239, see `task-4-fix-report.md`).
- [x] **Step 5: Commit** — `git commit -m "feat(wisdom): two-stage decision core — rules (i)/(ii), structural v1-merge, §4 fallback matrix"` → 8b67580, review-fix round AMENDED to **c602357** (+885/−18; review verdict APPROVED, five fixes applied).

## Task 5: CLI wiring — `--two-stage` flag, labels, audit line, embed-skip, query-time coverage

**Files:**
- Modify: `tools/src/queries.rs` (`wisdom_match_cmd` :768; embed-skip :791; query_text_for_scheme call :788)
- Modify: `tools/src/bin/ct.rs` (`WisdomCmd::Match` :354-366 gains flags; dispatch :702-715)
- Modify: `src-tauri/src/wisdom_match.rs` (new `wisdom_match_two_stage` orchestration pub fn, called by the CLI; labels + audit emitted here)
- Test: `tools/tests/cli_wisdom_two_stage.rs` (new integration test; stub embedder via `CURATED_EMBED_STUB`)

**Interfaces:**
- Consumes: `two_stage_entries`, `TwoStageOutcome`, `two_stage_k` (Task 4); `candidate_set`/`coverage` probes (Task 3); `gate_model_key`; `wisdom_match_in_scheme` (:209); `embed_one(profile, text: String) -> Result<Vec<f32>>` (`src-tauri/src/embedder/mod.rs:207`); `retrieval::load_embed_profile`.
- Produces:
  - CLI: `ct wisdom match [--two-stage] [--single-stage] [existing flags] <text>` (flags are mutually exclusive).
  - Env kill-switch: `CURATED_WISDOM_SINGLE_STAGE=1` (§5).
  - STDOUT label when active AND two-stage actually ran: `semantic-v2-two-stage:{key}` (§5). v1 fallback prints the existing v1 label.
  - STDERR audit line on EVERY path where two-stage was requested, including fallbacks and uncalibrated no-opens (r29-m4): `wisdom_two_stage_audit rule=<i|ii|none> open=<0|1> top=<score or none> v1merged=<n> floor=<key> k=<k> stale_embed_key=<n>` — exactly seven fields, this order, space-separated, single line.
  - The pinned 5-field `wisdom_gate_decision` stderr line: byte-identical under both paths.
  - `pub fn wisdom_match_two_stage(conn, profile, query, scheme, max, exclude, two_stage: bool, now_ms) -> Result<WisdomMatch>` in `src-tauri/src/wisdom_match.rs` — the shared entry point (Task 14/15's driver calls this directly).

- [x] **Step 1: Write the failing tests**

```rust
// tools/tests/cli_wisdom_two_stage.rs  (pattern: existing CLI integration tests
// in tools/tests/ — Brain::temp scratch brain + CURATED_EMBED_STUB=stub:constant8)
#[test]
fn two_stage_flag_wires_through_and_emits_audit_line() { /* run ct wisdom match --two-stage --json "q";
    assert stdout has no semantic-v2-two-stage label when floor missing (v1 fallback) and
    stderr contains wisdom_two_stage_audit with 7 fields and rule=none */ }

#[test]
fn pre_v28_readonly_brain_degrades_to_v1_exit_0() { /* r28-M3: V27-shape fixture brain, open RO,
    --two-stage ⇒ exit 0, v1 label, stale_embed_key=<candidate count>; never "no such column" */ }

#[test]
fn wisdom_gate_decision_stderr_line_byte_identical() { /* capture stderr under --two-stage and
    --single-stage; assert the 5-field wisdom line equal in shape (regex) and both parse to
    the same 5 fields */ }

#[test]
fn single_stage_flags_and_env_are_kill_switches() { /* --single-stage and CURATED_WISDOM_SINGLE_STAGE=1
    both suppress the audit line entirely and the label */ }

#[test]
fn embed_skip_when_both_floors_missing() { /* stub counts embed calls (counting stub); both floors
    absent ⇒ 0 embeds; v1-floor-only ⇒ exactly 1 embed (§3 embed-skip, §4) */ }

#[test]
fn raw_scheme_reuses_one_embed_per_message() { /* scheme raw: exactly ONE embed (stage-1 vector IS
    the fact vector, r26 m3); instrument via the counting stub */ }
```

- [x] **Step 2: Run to verify FAIL.**
- [x] **Step 3: Implement.** In `wisdom_match_cmd` (:768): after resolving profile + scheme, when `two_stage` active (flag XOR env) call `wisdom_match_two_stage`; else the existing path. `wisdom_match_two_stage` order of operations (pinned):
  1. Probe schema: `two_stage::two_stage_schema_ready` false → v1 fallback path (label v1; audit `rule=none ... stale_embed_key=<count via probe 0>`), never error (r28-M3).
  2. Resolve floors: `v1_floor = gate_floor(gate_model_key_for_scheme-or-current-key ...)`, `ts_floor = gate_floor(&format!("{}:two-stage:{}", floor_key_for(key, scheme), calibrated_rule))` — during harness runs the rule is passed explicitly; shipped default reads the composite key for each rule in pinned order `restricted-cosine` then `chunk-hop`.
  3. Coverage check BEFORE any embed (§6): `coverage(conn, &gate_model_key(profile, stub))` — `None` ⇒ fallback; `Some((n,c))` with `c < n` ⇒ fallback with `stale_embed_key = n - c`; `n == 0` ⇒ vacuously covered.
  4. Raw embed (stage 1) — skipped only per §4 both-missing rule (embed-skip at :791 gains the both-keys condition):
  ```rust
  // BEFORE (today): if wm::gate_floor(&scheme_key).is_none() { skip embed; uncalibrated }
  // AFTER:
  let v1_missing = wm::gate_floor(&v1_key).is_none();
  let ts_missing = BOTH composite keys missing;
  if v1_missing && ts_missing { /* uncalibrated: skip ALL embeds, audit line, return */ }
  ```
  5. Under `Scheme::Raw`, reuse the raw vector as the schemed vector (r26 m3 — ONE embed); under `Instr1`, embed `query_text_for_scheme(query, scheme)` a second time (:788 call site already builds the prefixed text).
  6. Call `two_stage_entries`, print the stdout label `semantic-v2-two-stage:{key}` only when rule ran; emit the audit line on every requested path; print the pinned 5-field line EXACTLY as today with the merged entries.

- [x] **Step 4: Run to verify PASS.** `cargo test --manifest-path tools/Cargo.toml -- --test-threads=1 two_stage` + full both-manifest suites — run 2026-10-16 (fix round included): `cli_wisdom_two_stage` 8/0 · `ct_wisdom_match` 10/0 · `wisdom_match` lib 43/0 · FULL src-tauri (`--features test-utils,mcp-server`) 1704/0 · FULL tools 202/0, both exits 0.
- [x] **Step 5: Commit** — `git commit -m "feat(cli): ct wisdom match --two-stage — labels, seven-field audit line, embed-skip, coverage-first ordering"` → 2a1804b, review-fix round AMENDED to **dbb854f** (+1055/−23). Review ladder: sor Approved; task-reviewer "Needs fixes" (Important #1 coverage-before-embed, #2 signature deviation, 3 Minors) → controller adjudication (fix #1 via pub precheck; #2 blessed-with-precheck; minors fix) → fix round applied; controller verified the fix shapes in code + tools suites re-run green (lib re-verification: 43/0 per fix report).

## Task 6: Benchmark freeze coverage — `replay_two_stage` + per-key freeze assertion

**Files:**
- Modify: `src-tauri/tests/wisdom_gate_bench.rs` (per-key hard-coded tests :22-44; floor assertion :107-111)
- Create: freeze dir `src-tauri/tests/fixtures/wisdom_gate_bench/<winning-or-placeholder-key>/` — NOTE: populated in Task 15 from calibration output; this task adds the TEST plus a stub-key fixture dir so the assertion compiles and passes pre-calibration.

**Interfaces:**
- Consumes: `gate_floor` lookup semantics (composite key must resolve, `wisdom_gate_bench.rs:107-111`); `WISDOM_GATE_FLOORS` (Task 4 adds two-stage keys ONLY at flip-to-default, Task 15).
- Produces: `#[test] fn replay_two_stage()` (reads `expected.json` from its freeze dir like the existing replay tests) and `#[test] fn every_non_stub_floor_key_has_a_freeze_dir()`.

- [x] **Step 1: Write the failing tests**

```rust
#[test]
fn every_non_stub_floor_key_has_a_freeze_dir() {
    // for (key, _) in wisdom_match::WISDOM_GATE_FLOORS, skip keys containing "stub",
    // assert tests/fixtures/wisdom_gate_bench/<sanitized key>/expected.json exists.
}
#[test]
#[ignore] // unignored by Task 15's flip-to-default (floor value comes from the artifact)
fn replay_two_stage() {
    // Reuses the existing per-key replay harness in this file, but the freeze
    // dir is the two-stage composite key dir and the floor assertion is
    // gate_floor(&composite_key) == expected.floor (the composite-key lookup
    // itself is unchanged — spec §3).
}
```

- [x] **Step 2: Run to verify FAIL** (no composite key in `WISDOM_GATE_FLOORS` yet ⇒ `gate_floor` lookup misses). Create the freeze dir now with the `expected.json` SCHEMA (keys only): `{"floor": 0.0, "rule": "restricted-cosine", "k": 8, "probes": []}`. The floor VALUE is genuinely decided by calibration (Task 15): because the composite key is absent from `WISDOM_GATE_FLOORS` pre-calibration, `replay_two_stage` is `#[ignore]`d until Task 15 flips it, and THIS task adds the explicit CI step that runs that ignored test (Global Constraints rule). `every_non_stub_floor_key_has_a_freeze_dir` runs always and passes now.

- [x] **Step 3: Implement** the two test fns + fixture dir with a minimal `expected.json` schema documented in the dir's README line inside `expected.json` (`{"floor": <f32>, "rule": "restricted-cosine"|"chunk-hop", "k": <usize>, "probes": [...] }`).
- [x] **Step 4: Verify PASS** (non-ignored coverage test).
- [x] **Step 5: Commit** — `git commit -m "test(bench): two-stage freeze-dir coverage assertion + ignored replay_two_stage (unignored at flip-to-default)"` → a02affc. Review ladder: sor Approved (1 finding = the plan-mandated ci.yml `|| true`+marker-grep guard, sev 1.56/is_real 0.64); task-reviewer **Approved** (sa-0-e5c54be9): RED re-verified live (`replay_two_stage` 1 failed), green re-run live (3/0/1-ignored), all 7 deviations verified legitimate. 3 Minors adjudicated: #1 plan-mandated guard can't distinguish wrong-reason failures pre-flip — ACCEPTED, rot window bounded to Task 15 (sor concurred); #2 f64→f32 double-rounding `assert_eq!` on floor (Task 15 will surface if it ever bites) — ACCEPTED, pins same pattern as existing per-key tests; #3 hard-coded historical key→dir mappings in `freeze_dir_for` — ACCEPTED, forced by on-disk v1 history + doc comment. No fix round.

---

# Task Group 2 — Safety + bootstrap (spec §9 step 2)

## Task 7: Chunker fingerprint constant + funnel stamp read

**Files:**
- Modify: `src-tauri/src/chunker/mod.rs` (top, after imports)
- Test: `src-tauri/src/chunker/mod.rs` tests module

**Interfaces:**
- Consumes: nothing (greenfield constant; the chunker has NO fingerprint today — verified: zero `fingerprint` hits in the repo at bc83d06).
- Produces: `pub const CHUNKER_FINGERPRINT: &str` (Task 8's decision table + Task 13's verify-scratch + the stamp write/read in both consume it). Semantics: a short stable string identifying the chunking ALGORITHM+params (e.g. `"v1"` initially); bumping it must be a deliberate code change.

- [x] **Step 1: Failing test**

```rust
#[test]
fn chunker_fingerprint_is_stable_and_nonempty() {
    assert!(!crate::chunker::CHUNKER_FINGERPRINT.is_empty());
    assert_eq!(crate::chunker::CHUNKER_FINGERPRINT, "v1");
}
```

- [x] **Step 2: Verify FAIL** (constant missing).
- [ ] **Step 3: Implement**

```rust
/// Stable identity of the chunking algorithm + parameters. Written into the
/// funnel stamp (spec §8(c), §6 — the stamp is CHUNKER-ONLY); bump on any
/// change that alters produced chunk boundaries or content hashes.
pub const CHUNKER_FINGERPRINT: &str = "v1";
```

- [x] **Step 4: Verify PASS.**
- [x] **Step 5: Commit** — `git commit -m "feat(chunker): CHUNKER_FINGERPRINT constant for the funnel stamp"` → 7011c08d (executed inline by controller; see task-7-report.md deviations). Review ladder: sor Approved (0 findings, risk 0.68); task-reviewer **Approved** (sa-1-b255799e): verbatim plan implementation, single commit + exact message verified, tree/HEAD consistency checked. 1 Minor (constant placed before `use std::path::Path;` vs "after imports" note) — ACCEPTED as cosmetic. No fix round.

## Task 8: Diff-swap transaction in `ingest_file_virtual` (+ `mark_document_indexed` hash param, `embed_key` writer, `IngestRefused`, four-row decision table)

This is the largest task; it lands as several commits inside one task.

**Files:**
- Modify: `src-tauri/src/pipeline/mod.rs` (`ingest_file_virtual` :644-766: short-circuit :691-694 stays; delete-then-embed :695, :748-763 replaced by the swap; `upsert_document` for existing docs moves inside the swap txn)
- Modify: `src-tauri/src/db/queries.rs` (`mark_document_indexed` :91-103 gains `indexed_hash: &str`)
- Test: `src-tauri/src/pipeline/mod.rs` tests module

**Interfaces:**
- Consumes: `gate_model_key` (§6 embed_key value); `compute_chunk_hash` (`src-tauri/src/db/chunk_hash.rs:18`); `delete_document_chunks`/`insert_chunk`/`insert_embedding`/`upsert_document`/`get_document_by_path` (`src-tauri/src/db/queries.rs:50,55,82,10,24`); `CHUNKER_FINGERPRINT` (Task 7); `Connection::unchecked_transaction` (pattern: `src-tauri/src/lib.rs:2444`) — but the SWAP uses `TransactionBehavior::Immediate` via `Transaction::new_unchecked(conn, TransactionBehavior::Immediate)` (same shape the §8(b) heal fix uses).
- Produces:
  - `pub fn mark_document_indexed(conn: &Connection, doc_id: i64, indexed_hash: &str) -> Result<()>` — sets `status='indexed', last_indexed=unixepoch(), last_indexed_hash=?2`. The ONLY writer of `last_indexed_hash` (rev 12 m3). ALL existing callers updated (pipeline swap txn, any other call sites — `grep -rn "mark_document_indexed(" src-tauri tools` and fix each).
  - `pub struct IngestRefused;` (`impl std::fmt::Display + std::error::Error`) — the typed refusal error (§8(c); SINGLE struct, no enum).
  - Inside `ingest_file_virtual`, the FOUR-row decision table (evaluated AFTER the :691-694 short-circuit; zero-chunk docs and content edits bypass the whole table):
    | forced? | chunker current? | behavior |
    |---|---|---|
    | yes | yes | pass (full re-embed + `embed_key` write) |
    | yes | stale/missing | per-doc LOSSLESS check: rechunk in memory; new hash-set == stored set ⇒ PROCEED; != ⇒ refuse (`Err(IngestRefused)`) |
    | no | current | no-op success — `pending` ROWS ONLY: mark `indexed`, restore `hash=last_indexed_hash`; `pending_reindex` rows EXCLUDED (r28-M2: untouched, stay sweepable). Key-currency condition: `embed_key` stale/NULL ⇒ re-embed STORED CHUNKS in place + update key (repair-embed-key mechanics); on embed failure apply the refusal reset (r29-m3) |
    | no | stale/missing | same no-op-success row as above |
    "Chunker current" = stamp's chunker component == `CHUNKER_FINGERPRINT` (stamp is CHUNKER-ONLY; no model component exists, r25-M3).
  - Refusal row-state reset INSIDE `ingest_file_virtual` (r27 m3): on drift-refusal, `UPDATE documents SET status='indexed', hash=last_indexed_hash WHERE status IN ('pending','pending_reindex') AND id=? AND last_indexed_hash IS NOT NULL` (one statement).
  - Swap-race rule: re-read chunk hashes UNDER the IMMEDIATE lock; abort (clean retryable error) on any mismatch vs the pre-embed read.
  - Duplicate-embeddings guard: inside the swap txn, after the write set, `DELETE FROM embeddings WHERE rowid NOT IN (SELECT MAX(rowid) FROM embeddings GROUP BY chunk_id) AND chunk_id IN (SELECT id FROM chunks WHERE doc_id=?1)` (rowid-max kept, CodeRabbit rev-33).
  - Per-doc unforced-edit skip: unchanged-hash chunks skip re-embed ONLY when `documents.embed_key == gate_model_key(profile, stub)`; NULL/stale ⇒ full re-embed (rev-24 MAJOR-2).

- [x] **Step 1: Write failing tests**

```rust
#[test]
fn diff_swap_preserves_unchanged_chunks_and_reembeds_all_on_forced() { /* forced pass: unchanged-hash
    chunk keeps its chunk_id, embeddings row UPDATEd in place; changed-hash inserted; removed deleted;
    documents.embed_key == current key after swap */ }

#[test]
fn embed_failure_leaves_prior_state_entirely_for_existing_doc() { /* stub 401 mid-swap: status, hash,
    chunks, embed_key ALL the old values; NOT moved to error (r7 MINOR-1) */ }

#[test]
fn new_doc_embed_failure_marks_error_pending() { /* new doc: mark_document_error path intact */ }

#[test]
fn swap_race_aborts_on_hash_mismatch_under_lock() { /* mutate a chunk content_hash between pre-read
    and swap; assert clean retryable Err, no partial writes */ }

#[test]
fn duplicate_embeddings_rows_are_collapsed_by_swap() { /* two embeddings rows for one chunk ⇒ exactly
    one row, rowid-max, new-model vector; semantic_search returns chunk once */ }

#[test]
fn forced_drift_refusal_is_typed_and_resets_pending_rows() { /* stamp stale + unchanged bytes +
    changed stored hash set ⇒ Err(IngestRefused); a pending/pending_reindex row resets to indexed
    with hash=last_indexed_hash; an indexed row stays indexed */ }

#[test]
fn forced_lossless_model_swap_proceeds_without_stamp() { /* stamp MISSING, profile key changed,
    hash set identical ⇒ PROCEED, embed_key = new key (r30-M1) */ }

#[test]
fn nonforced_noop_only_on_pending_rows_excludes_pending_reindex() { /* r28-M2 + r28-M2 key-currency:
    pending row + current key ⇒ indexed + hash restored; pending_reindex row ⇒ untouched, sweepable;
    pending row + stale key ⇒ stored chunks re-embedded in place, key updated */ }

#[test]
fn unforced_edit_skip_requires_per_doc_key_match() { /* NULL embed_key ⇒ full re-embed of unchanged
    chunks (rev-24 MAJOR-2) */ }
```

- [x] **Step 2: Verify FAIL.**
- [x] **Step 3: Implement.** Restructure `ingest_file_virtual` (:644-766): keep :653-690 gates + :688 hash read; short-circuit :691-694 untouched; then the decision table; the swap body:

```text
PRE (outside txn): read file bytes, chunk (in memory), compute new content_hash set,
                   read stored (chunk_id, content_hash) set + documents row state,
                   embed ALL chunks (forced) or only changed/unembedded (unforced w/ per-doc key rule)
SWAP (ONE IMMEDIATE txn):
  - if existing doc: upsert_document (new hash, pending) INSIDE txn (rev-5 M2)
  - re-read stored chunk hash set under the lock; mismatch vs pre-read ⇒ ROLLBACK + retryable Err
  - unchanged-hash chunks: UPDATE embeddings in place (never bare INSERT)
  - changed-hash: insert new chunks (+embeddings); removed (incl. empty-hash rows): delete chunks
  - duplicate-embeddings cleanup (rowid-max kept)
  - UPDATE documents SET embed_key = <key actually used>   -- §8(a) primary writer
  - mark_document_indexed(&tx, doc_id, &hash_of_bytes_actually_chunked)
COMMIT
On embed failure BEFORE swap opens: existing doc ⇒ log + return Ok (state intact);
new doc ⇒ mark_document_error (row exists from pre-embed upsert, which STAYS outside for new docs).
```

- [x] **Step 4: Verify PASS** (pipeline tests + full suite).
- [x] **Step 5: Commit(s)**

```bash
git commit -m "feat(db): mark_document_indexed gains indexed_hash param (sole last_indexed_hash writer)"
git commit -m "feat(pipeline): diff-swap transaction with embed_key write, swap-race recheck, duplicate guard"
git commit -m "feat(pipeline): four-row funnel decision table + typed IngestRefused + refusal row reset"
```
→ 4 commits: d40d3001 (c1), 4517f860 (c2), e3cff345 (c3, amended once — includes the §8(a)-mandated semantics-coupled update to tests/pipeline.rs, D7), aa6fbb85 (c4: two missed tools/tests fixture callers, D6 — separate honest fix commit within the 3-4 allowance). Review ladder: sor per commit — c1 Approved, c2 "Changes requested", c3 Approved, c4 Approved (0 findings); task-reviewer adjudicated ALL 11 findings: **0 real defects, 9 false-positives, 2 accepted-with-reason** (pipeline/mod.rs:1618 test-gap = D3, no deterministic seam without new production surface — CARRIED AS DEBT to Task 9+; tests/pipeline.rs:174 = D7 plan-mandated flip). **Approved, no fix round.** Controller post-adjudication verification: --lib diff_swap 9/0, --test pipeline 5/0, tools full 202/0, exits 0; implementer's full src-tauri 1714/0/1 consistent with the 1705/0/1 gate (+9 new tests). Deviations D1-D8 in task-8-report.md; fmt disclosure controller-audited as pure rewraps. Carried to Task 9: removable `#[allow(dead_code)]` on IngestRefused, D8 report-text cleanup, D3 coverage (injectable pre-swap hook or unit-testable hash-set helper).

## Task 9: `IngestRefused` callers — ct ingest, bulk_reindex skip, pipeline worker log, defensive kick downcast

**Files:**
- Modify: `tools/src/cmds.rs` (:172-178 forced ingest loop, :180 accounting)
- Modify: `tools/src/bin/bulk_reindex.rs` (:129-130 `?` abort)
- Modify: `src-tauri/src/pipeline/mod.rs` worker loop (:257-261 generic Err arm)
- Modify: `src-tauri/src/wisdom_deposit.rs` (:379-388, defensive downcast BEFORE the `map_err` at :387)
- Test: `tools/tests/` + `src-tauri/src/pipeline/mod.rs` tests

**Interfaces:**
- Consumes: `IngestRefused` (Task 8).
- Produces: caller contract — a refusal is NEVER a generic `Err`: (1) `ct ingest` ⇒ success (exit 0, SKIP in loop accounting: no `failed += 1`, file still in linker entity set); (2) `bulk_reindex` ⇒ SKIP: one informational stderr line, pass CONTINUES, linker still runs over processed entity_ids; (3) pipeline worker ⇒ ONE informational stderr line, NOT `write_error_log`, continue loop; (4) `wisdom_deposit` kick ⇒ defensive downcast before `map_err`, treated as success, still records `chunked` (unreachable today — force=false at :384 — belt-and-suspenders).

- [x] **Step 1: Failing tests**

```rust
#[test]
fn worker_logs_ingest_refused_without_write_error_log() { /* force-refuse via stale stamp + unchanged
    bytes; assert stderr has exactly ONE informational line, error log count 0, loop continued */ }
#[test]
fn bulk_reindex_counts_refusal_as_skip_and_continues() { /* two paths, first refused; assert second
    processed, linker ran over first doc's entity too, exit 0 */ }
#[test]
fn ct_ingest_treats_refusal_as_skip() { /* loop accounting: failed == 0, exit 0 */ }
```

- [x] **Step 2: Verify FAIL.**  **Step 3: Implement** the four downcasts per the contract above (`downcast_ref::<IngestRefused>()` FIRST in each arm).
- [x] **Step 4: Verify PASS.**
- [x] **Step 5: Commit** — `git commit -m "feat(pipeline): IngestRefused downcasts at all four callers (skip semantics, no error-log storm)"`

> **Adjudication (controller, 2026-10-16):** commit cb15a737 on 045b8894 (+555/−15, 6 files). RED verbatim first (task-9-red.md 17:59:07Z); suites src-tauri **1717/0/1**, tools **204/0** (controller-summed, exits 0; 1 ignored = pre-existing node-gated installed_engine_setup). sor: Approved (risk 2.04/3). Task-reviewer: ✅ spec-compliant (⚠️ serial/fmt unverifiable from diff), **Approved, no fix round**; sor findings adjudicated: worker arm = no real bug (return exits per-job closure only; pending decrement outside), wisdom_deposit defensive downcast correct-as-written (unreachable today, per brief), cmds.rs test-gap partially real → subsumed by carried debt. **CARRIED DEBT (test-strength, non-blocking):** (1) worker test's errors.log assertion doubly vacuous (vault_root=None ⇒ write_error_log no-op; TempDir dropped before parent check) — make real by passing `Some(tmp.path())`; (2) bulk/ct linker edge assertions pre-satisfied by seed — bulk refusal skips `entity_ids.insert` (contract-consistent: bulk binds "over *processed* entity_ids" vs ct's "file still in linker entity set" — deliberate wording divergence, behavior matches contract as written). Surface both at whole-branch review (Task 15/16). Deviations D-A/D-B/D-C/D-E/D-F in task-9-report.md; D3 via `swap_race_check()` helper + 2 unit tests; D8 `#[allow]` removed, no warning.

## Task 10: Cross-process deletion breaker — heal + regrade + `ct heal` flags

**Files:**
- Modify: `src-tauri/src/db/heal.rs` (`heal_invalid_sources_conn` :46, per-row txn :80)
- Modify: `src-tauri/src/lib.rs` (`heal_lost_librarian_inferred` :2403; breaker site :2440; `Transaction::new_unchecked(conn, TransactionBehavior::Immediate)` + grounding check INSIDE txn + conditional UPDATE guard)
- Modify: `src-tauri/src/db/evidence_regrade.rs` (`purge_doomed_rows` :108-146: pre-delete `|doomed| >= L` check + budget check; :134 txn → IMMEDIATE; in-txn recount vs pre-txn doomed ID SET, abort on mismatch)
- Modify: `tools/src/bin/ct.rs` (`Heal` variant :102 gains `--reset-breaker`, `--allow-bulk`; `evidence regrade` gains `--allow-bulk` — note regrade is a DIFFERENT `Cmd` variant (`Regrade` :284, dispatch arm `EvidenceCmd::Regrade` :647), not part of `Heal`; thread the flag through `cli_common::evidence_regrade_cmd`'s signature)
- Test: per-module tests

**Interfaces:**
- Consumes: `llm_wiki_meta (key, value)` table; `hard_delete_entries` (`src-tauri/src/db/commit.rs`); `source_ref_is_still_grounded` (`commit.rs`); `evidence_has_live_chunk` (`commit.rs:666`).
- Produces (shared by heal + regrade):

```rust
// src-tauri/src/db/breaker.rs (NEW module)
pub const BREAKER_L_HEAL: &str = "breaker_L_heal";
pub const BREAKER_L_REGRADE: &str = "breaker_L_regrade";
pub fn heal_threshold(conn: &Connection) -> Result<i64>;    // max(ceil(0.05*L), 10), L = heal's selection
pub fn regrade_threshold(conn: &Connection) -> Result<i64>; // L = librarian_evidence JOIN live entries count
/// remaining = own_threshold − shared_spent; tripped epoch does NOT roll over.
/// |doomed| >= L ⇒ refuse regardless of budget (rev-34 Small-L rule).
pub fn breaker_check(conn: &Connection, doomed_count: usize, own_l: i64, own_threshold: i64)
    -> Result<BreakerVerdict>; // Allow | Refuse { reason }
/// increment spent INSIDE each per-row IMMEDIATE txn (shared counter key).
pub fn breaker_spend(conn: &Connection, n: i64) -> Result<()>;
/// ct heal --reset-breaker: delete epoch ts + spent + both breaker_L_* in ONE IMMEDIATE txn.
pub fn breaker_reset(conn: &Connection) -> Result<()>;
```

- [x] **Step 1: Failing tests** (from §10): threshold math `max(⌈0.05·L⌉,10)` both denominators; cross-process budget (two `Connection`s on one file, shared refusal); all three writers covered; Small-L rows — `L=7,|doomed|=7 ⇒ refuse; L=3,|doomed|=3 ⇒ refuse; L=50,|doomed|=50 ⇒ refuse; L=50,|doomed|=4 ⇒ proceeds`; tripped epoch does not roll over (24h rollover only for healthy epochs); regrade batch all-or-nothing + in-txn classification recount vs ID SET with abort on mismatch; re-anchor UPDATEs commit regardless of verdict; `--reset-breaker` clears keys; `--allow-bulk` skips the breaker for one run; while tripped, one stderr line EVERY run.
- [x] **Step 2: Verify FAIL.**
- [x] **Step 3: Implement** — the lib.rs fix exactly per spec (§8(b)): `heal_lost_librarian_inferred` moves to IMMEDIATE (`Transaction::new_unchecked`), `source_ref_is_still_grounded` INSIDE the txn, conditional `UPDATE ... WHERE deleted_at IS NULL AND source_ref = ?`; regrade purge txn at :134 → IMMEDIATE; epoch keys `breaker_L_heal`/`breaker_L_regrade` snapshotted under their OWN keys; tripping writer's refusal line names which threshold was hit.
- [x] **Step 4: Verify PASS.**
- [x] **Step 5: Commit** — `git commit -m "feat(db): cross-process bulk-deletion breaker (shared budget, |doomed|>=L rule, ct heal --reset-breaker/--allow-bulk)"`

> **Adjudication (controller, 2026-10-16, TWO-ROUND ladder):** main commit **f07eaaff** on daa7a8b2 (10 files, +1529/−23) + fix commit **52ebfcf3** (4 files, +273/−8). History: implementer agent hit iteration cap post-implementation → finisher agent completed gates + fixture remediation. Gates: src-tauri **1735/0/1** (1717 baseline + 15 task tests + 3 fix-round tests), tools **205/0** (204 + 1). Round 1: sor INCOMPLETE (40/61 clusters; verdict not sustained by reviewer) → task-reviewer **Needs fixes**: cross-writer rollover hole (trip-ness inferred per-writer; persisted breaker_tripped marker required — spec pins "marked tripped when ANY writer refuses"), recount-mismatch abort + allow-bulk behavioral tests missing. Rulings: 5-arg breaker_check ACCEPTED (spec rev-5 per-writer L keys; brief sketch predates); fixture edits necessity-driven (10 pre-existing lone-doomed-row tests fixed via grounded-companion, refusal-arm tests deliberately keep bare fixture; 2 count assertions updated arithmetically — legitimate). Round 2 re-review: **✅ all findings resolved, Fix round Approved** — marker set on all 3 refusal arms, consulted before rollover, cleared by breaker_reset; cross-writer test RED-verified; recount-mismatch test asserts guard-controlled outcomes (mutation-RED); allow-bulk pinned both arms. Remaining known gap (accepted): one cosmetic refusal-string wording change, no contract pinned. Minor stylistic items (refusal-path export deletion debate, epoch-bookkeeping rollback asymmetry) documented as spec/ops notes. Deviations in task-10-report.md; incident: fix-round gates initially blocked by 100%-full disk, resolved via regenerable-cache cleanup (untracked only).

## Task 11: `librarian_evidence_seen` marker (trigger + migration backfill)

**Files:**
- Modify: `src-tauri/src/db/connection.rs` (V28 body — the SAME ungated-idempotent migration chain, runs AFTER V18's DDL; add `librarian_evidence_seen` trigger + backfill to `apply_v28_two_stage_columns` or a sibling fn called beside it)
- Test: `src-tauri/src/db/connection.rs` tests

**Interfaces:**
- Consumes: Task 1's migration fn (extend it; V18 DDL already exists — trigger creation must come after `librarian_evidence` exists, which every post-V18 brain has; for pre-V18 replicas the migration must NOT fail).
- Produces: meta key `librarian_evidence_seen = '1'`; `AFTER INSERT` trigger on `librarian_evidence` with `INSERT OR IGNORE INTO llm_wiki_meta` body (idempotent); migration backfills the key iff rows exist today; the key is explicitly KEPT by the clear transaction (NOT in any purge list — verified: the clear path never deletes `llm_wiki_meta` (`db/queries.rs:244` "survives the clear"); verify by inspection only, NO code change needed — the plan's test at Step 1 already asserts the marker survives a meta round-trip).

- [x] **Step 1: Failing tests**

```rust
#[test]
fn evidence_seen_marker_armed_by_trigger_and_backfill() {
    let conn = open_in_memory().unwrap();
    insert_live_evidence_row(&conn); // AFTER migration: trigger fires
    let v: String = meta_get(&conn, "librarian_evidence_seen").unwrap();
    assert_eq!(v, "1");
    // backfill arm: a brain with rows but no marker gets it on next migrate
    meta_del(&conn, "librarian_evidence_seen");
    crate::db::connection::migrate_open_db(&conn, None).unwrap();
    assert!(meta_get(&conn, "librarian_evidence_seen").is_some());
}
#[test]
fn pre_v18_fixture_open_succeeds() { /* V17-shape fixture (no librarian_evidence table):
    migrate must NOT fail creating the trigger; marker absent = fresh per definition */ }
```

- [x] **Step 2: Verify FAIL.**  **Step 3: Implement** — inside `apply_v28_two_stage_columns`'s locked section, AFTER confirming `librarian_evidence` exists (probe `sqlite_master`; skip trigger creation when absent — pre-V18 replicas get it on a later open): `CREATE TRIGGER IF NOT EXISTS librarian_evidence_seen_trg AFTER INSERT ON librarian_evidence BEGIN INSERT OR IGNORE INTO llm_wiki_meta (key, value) VALUES ('librarian_evidence_seen','1'); END;` plus the one-shot backfill `INSERT OR IGNORE INTO llm_wiki_meta SELECT 'librarian_evidence_seen','1' WHERE EXISTS (SELECT 1 FROM librarian_evidence)` — inside the ALTER-call transaction only.
- [x] **Step 4: Verify PASS.**
- [x] **Step 5: Commit** — `git commit -m "feat(db): librarian_evidence_seen fresh-brain marker (AFTER INSERT trigger + one-shot migration backfill)"`

> **ERRATA (controller, 2026-10-16, per Task-11 review):** Step 3's "inside the ALTER-call transaction only" is WRONG for the marker backfill — it contradicts this task's own Step-1 contract test (a strictly-ALTER-gated backfill could never re-arm after marker deletion on an already-migrated brain) and the spec (spec:1424-1427 says only "backfilled by the new ungated migration"; the pinned one-shot sentence at spec:1471-1487 names only the embed_key/last_indexed_hash backfills). As built: the backfill rides a migration transaction **gated on marker-pending** (rows exist AND marker absent) — self-disarming, write lock taken at most once per brain (production never deletes the key; clear keeps it, now test-pinned). The V27 one-shot constraint applies to the column backfills, not this meta insert. Also note: the second contract test landed as `pre_v18_fixture_open_succeeds_and_arms_marker_on_upgrade` (exact-prefix superset of the sketch name). Do NOT "fix" the gate back into impossibility.

> **Adjudication (controller, 2026-10-16):** commit **2bedb35d** on 94e11e41 (2 files, +228/−10). Gates: src-tauri **1737/0/1** (=1735+2), tools **205/0**; RED verbatim first; sor Approved (risk 1.81/3). Reviewer: ✅ spec-compliant, **Approved, no fix round**; backfill-gating deviation ACCEPTED-with-reason (see errata above); clear-path survival now test-pinned (keep-count 3→4); pre-V18 safety tested both directions. MINORS CARRIED as debt (whole-branch review): (1) dead first `let trg` binding in the pre-V18 test (shadowed; likely test-profile warning — assert `trg == 0` to strengthen); (2) stale "V27 precedent" comment in test 1 (align with production comment); (3) undeclared test-name extension (declared here).

## Task 12: `ct reindex` subcommand skeleton + `repair-embed-key --stale`

**Files:**
- Modify: `tools/src/bin/ct.rs` (`enum Cmd` :14 — add `Reindex { #[command(subcommand)] cmd: ReindexCmd }` beside `Heal` :102; `enum ReindexCmd { RepairEmbedKey { #[arg(long)] stale: bool } }`; dispatch arm)
- Create: `tools/src/reindex.rs` (impl; registered in tools crate like `queries.rs`)
- Test: `tools/tests/cli_reindex_repair.rs`

**Interfaces:**
- Depends on: Task 9 (refusal contract) — this task's smoke reads the breaker's stderr trip line, which Task 9's pipeline-worker log shape defines.
- Consumes: `resolve()`/`open_rw`/`open_ro` (`tools/src/write.rs`); `retrieval::{resolve_brain_paths, load_embed_profile}` (`bulk_reindex.rs:87-89` pattern); `gate_model_key(profile, stub)`; `TransactionBehavior::Immediate`; `two_stage::coverage` (Task 3) for reporting.
- Produces: `ct reindex repair-embed-key [--stale]` — re-embeds the STORED CHUNKS of every stale/NULL-key CANDIDATE doc directly from the DB (NO file read, NO status/tier precondition, NO stamp required); per doc ONE IMMEDIATE txn covering (a) in-place embeddings UPDATEs from stored chunk text and (b) `documents.embed_key = gate_model_key(profile, stub-as-used)`; the doc's chunk-id set RE-CHECKED UNDER THE LOCK against the pre-read (swap-race rule; mismatch ⇒ skip doc with a printed note, next run retries). Exit 0; prints per-doc verdicts + final coverage.

- [x] **Step 1: Failing test**

```rust
#[test]
fn repair_embed_key_stale_fixes_unreachable_doc() {
    // fixture brain (scratch Brain dir + stub profile): candidate doc with embed_key NULL
    // and its source file DELETED from disk ⇒ run `ct reindex repair-embed-key --stale` ⇒
    // doc's embed_key == stub key, vectors updated in place (same chunk ids), coverage 100%.
}
#[test]
fn repair_rechecks_chunk_ids_under_lock() { /* mutate a chunk id between read and txn (test hook or
    direct conn); assert doc SKIPPED, no partial write (vectors unchanged, key unchanged) */ }
```

- [x] **Step 2: Verify FAIL.**  **Step 3: Implement** the subcommand + re-embed-from-DB loop (embed via `embed_batch(profile, texts)` — `src-tauri/src/embedder/mod.rs:178`).
- [x] **Step 4: Verify PASS.**
- [x] **Step 5: Commit** — `git commit -m "feat(cli): ct reindex repair-embed-key --stale (DB-only per-doc repair under one IMMEDIATE txn)"`

> **Adjudication (controller, 2026-10-16, TWO-ROUND ladder):** main commit **8e64eec7** on 385fd99d (4 files, +646/−0) + fix commit **63e17474** (2 files, +119/−14). Gates: tools **208/0** (205 baseline + 2 task tests + 1 fix-round test), src-tauri **1737/0/1** (untouched by fix round); sor Approved (1.57/3). Round 1: reviewer **Approved** with named rulings — highest-id chunk seam LEGITIMATE (evidence topology; deleting the first chunk would drop the doc from the candidate set), embed_batch OUTSIDE the IMMEDIATE txn CORRECT (holding the write lock across a provider round-trip would make the under-lock re-check trivially always-pass — all writes are under the txn), removed scaffolding test ACCEPTABLE (near-tautological; test 1's in-process gate_model_key comparison carries the property). One Important carried to fix round: discovery-time race (doc deleted between candidate_set and per-doc read aborted the pass, exit 1 — contract violation). Round 2 re-review: **✅ all findings resolved, Fix round Approved** — per-candidate QueryReturnedNoRows catch (skip + continue, exit 0, note shape consistent), rows_affected==1 guard with 'doc_pass rollback, comment aligned; new seam CURATED_REPAIR_TEST_DELETE_DOC follows the accepted CURATED_REPAIR_TEST_RACE pattern (production-never-set, documented; no FK cascade — CT never sets the pragma). Remaining minors CARRIED as debt (whole-branch review): report overclaim on per-doc-failure continuation, dead now_ms param, `let _ = rw;` redundancy, raw CURATED_EMBED_STUB env reads (should use embedder::active_embed_stub), untested branches (count-mismatch skip, no-stored-chunks skip, write-error path).

## Task 13: `ct reindex verify-scratch` (bootstrap: chunk-only scratch rechunk + stamp write + sampled backfill)

**Files:**
- Modify: `tools/src/bin/ct.rs` (`ReindexCmd::VerifyScratch` variant)
- Modify: `tools/src/reindex.rs` (impl)
- Test: `tools/tests/cli_reindex_verify_scratch.rs`

**Interfaces:**
- Consumes: `CHUNKER_FINGERPRINT` (Task 7); `compute_chunk_hash` (content_hash of scratch rechunk); `evidence_has_live_chunk` (`commit.rs:666`) — scratch-ref resolution = for every live `librarian_evidence` evidence blob, its `content_hash` refs must resolve on the scratch rechunk (baseline 406/406) and every gate-eligible entry must stay hop-resolvable (baseline 365/365) — reuse Task 3's hop SQL against the scratch copy; `embed_one`/`embed_batch`; `cosine_similarity`; IMMEDIATE guarded per-doc writes (r28-m6).
- Produces: `ct reindex verify-scratch [--dry-run]`:
  1. Copy live DB to scratch; rechunk every doc on scratch under the current binary (CHUNK-ONLY — the ONLY embeds are the sampled backfill embeds).
  2. Chunker stamp: write iff every evidence hash ref resolves on scratch AND every gate-eligible entry stays hop-resolvable; stamp = CHUNKER-ONLY (`llm_wiki_meta` key `reindex_stamp` value `CHUNKER_FINGERPRINT`); a chunker bump FAILS the check and writes no stamp.
  3. Sampled backfill: per chunk-bearing doc, FIRST + LAST chunk re-embedded from STORED CHUNK TEXT (file existence + document status IRRELEVANT, r25-M1); match by cosine ≥ 1−1e−3 vs the stored vectors; on match, backfill `documents.embed_key` — the live write is GUARDED (r28-m6): each doc's write is ONE IMMEDIATE txn re-checking the doc's chunk-id set and current `embed_key` against the scratch snapshot; skip on mismatch. Failed sample ⇒ doc stays NULL/unchanged, PRINTED per doc.
  4. Prints per-doc verdicts + final candidate-set coverage; `--dry-run` writes NOTHING to either DB (print-only, r26 m6).

- [x] **Step 1: Failing tests**

```rust
#[test]
fn verify_scratch_writes_stamp_and_backfills_when_checks_hold() { /* fixture brain with live evidence +
    scorable chunks ⇒ stamp == CHUNKER_FINGERPRINT, embed_key backfilled on matching docs */ }
#[test]
fn verify_scratch_refuses_chunker_bump() { /* simulate drift (mutate scratch-rechunk hashes) ⇒
    no stamp written, exit non-zero, message names the lossless requirement */ }
#[test]
fn verify_scratch_dry_run_touches_nothing() { /* --dry-run: live DB byte-identical (hash the file
    before/after), scratch discarded */ }
#[test]
fn failed_sample_leaves_null_and_is_printed() { /* counting stub returns garbage for one doc ⇒ that
    doc NULL, verdict printed, coverage reflects it */ }
#[test]
fn guarded_live_write_skips_on_chunk_set_change() { /* mutate live doc's chunks after sampling ⇒
    write skipped, no mismatched stamping (r28-m6) */ }
```

- [x] **Step 2: Verify FAIL.**  **Step 3: Implement.**
- [x] **Step 4: Verify PASS.**
- [x] **Step 5: Commit** — `git commit -m "feat(cli): ct reindex verify-scratch — chunker-only stamp, sampled embed_key backfill, guarded live writes"`

> **Adjudication (controller, 2026-10-17, TWO-ROUND ladder):** main commit **feb4a311** on 7a581318 (3 files + RED evidence; all 5 verbatim tests GREEN). Gates: tools **213/0/0**, src-tauri **1737/0/1** (untouched), check --tests **0 warnings**, fmt clean. Round 1 review: **Changes requested** — Important: `--dry-run` was hollow (counts only, no analysis); Minors: scratch cleanup leaked on `?` paths (closure), guarded-write skip label conflated swap-race with already-backfilled, `entity_id_for_virtual_path` recomputed per chunk + skipped rechunks unsurfaced; plus test strengthening (4 dry-run assertions) and 2 report corrections (race seam = SAME live conn pre-txn; hop SQL necessarily drops the embeddings join on the chunk-only scratch). Round 2: fix commit **f33996e1** (+138/−46: full-analysis dry-run with NO live rw conn, ScratchGuard RAII, honest split skip labels, eid hoist + `rechunk_skipped` in summary, additive-only test diff); gates re-run green through the heavy-build wrapper (oomd had killed two bare runs); Tier-2 fix-delta re-review (glm-5.3, receipts-verified): **✅ Approved** — all 5 findings FIXED with quoted before/after, NO-WEAKEN confirmed (test names/line numbers identical, 0 deleted lines). Non-blocking Minors CARRIED as debt (whole-branch review): dry-run per-doc verdict prints "would backfill" where live would print "already backfilled" (scratch snapshot could predict it); ScratchGuard constructed one statement after `busy_timeout(?)` (negligible leak window); pre-existing src-tauri warning `unused variable: trg` at connection.rs:2068 (2bedb35d, not this task).

---

# Task Group 3 — Calibration harness (spec §9 step 3 / §7) — ships the harness, does NOT run calibration

## Task 14: Paired live calibration driver

**Files:**
- Create: `tools/src/bin/calibrate_two_stage.rs` (paired driver)
- Modify: `tools/src/reindex.rs` (expose scratch-fixture helpers reused from Task 13 — pub fns)
- Test: `tools/tests/calibrate_two_stage_harness.rs` (offline logic only — picker + artifact; live run is manual)

**Interfaces:**
- Consumes: `wisdom_match_two_stage` (Task 5) + `wisdom_match_in_scheme` (v1 arm, live label config) run against the LIVE brain (read-only `open_ro`); probes = the 150-probe real-traffic set as **JSONL** — the EXISTING `calibrate_wisdom_gate` format (`tools/src/bin/calibrate_wisdom_gate.rs:84` `read_jsonl::<Probe>`, serde `Probe` struct `:77`; reuse/extend that struct — do NOT invent a new schema), one line per probe with relevant-fact IDs; 40 relevant + 110 FP-probe stock per §7/r24 m7); `CURATED_EMBED_STUB` must be UNSET (refuses otherwise, like `calibrate_wisdom_gate.rs:8`).
- Produces: `calibrate_two_stage --probes <json> --rule <restricted-cosine|chunk-hop> --floor <f32> --k <usize> --baseline-key <v1 key> [--json-out <artifact path>]` — runs BOTH arms in the SAME invocation (same paired run, §7): baseline = the CURRENTLY SHIPPED default (named by its model/scheme key + floor in the artifact BEFORE the run, Opus r1 M3), two-stage arm via `--two-stage` semantics. Derives hit@2 + FP from probe labels + the pinned `open`/`top` audit fields (the gate cannot compute them, r25 m1). Writes the artifact JSON: both arms' hit@2/FP, letter verdict `two_stage_passes = ts_hit2 >= ss_hit2 && ts_fp <= ss_fp`, grid size, five stratified 75/75 held-out splits (informational), candidate-set coverage fraction at run time, per-message embed count + latency (r25 E6), rule-(ii) injection volume (r24 E7), merge attribution (r27 E4), v1-merge latency/volume (r31 E7), top-1 vs top-k ablation columns (r31 E5).

- [ ] **Step 1: Failing tests** (offline parts): letter verdict math (`ts_hit2 >= ss_hit2 && ts_fp <= ss_fp` — exact boundary: equal hit@2 + equal FP PASSES); artifact serialization round-trip; held-out split stratification is deterministic given a seed; refusal when `CURATED_EMBED_STUB` set; refusal when coverage < 100% recorded in artifact (two-stage arm must not run on a skewed set — the run aborts with instructions to run Task 13 then Task 12).
- [ ] **Step 2: Verify FAIL.**  **Step 3: Implement.**  **Step 4: Verify PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(calibration): paired live two-stage vs single-stage driver with acceptance-letter verdict + artifact"`

## Task 15: Rule/floor picker + flip-to-default gated on the acceptance letter

**Files:**
- Modify: `tools/src/bin/calibrate_two_stage.rs` (picker subcommand)
- Modify: `src-tauri/src/wisdom_match.rs` (`WISDOM_GATE_FLOORS` gains the winning composite key — THIS EDIT IS EXECUTED ONLY ON A PASSING LETTER; until then the flag stays opt-in)
- Modify: `src-tauri/tests/wisdom_gate_bench.rs` (un-`#[ignore]` `replay_two_stage`; populate its freeze dir `expected.json` from the artifact)
- Create: `scripts/calibrate-two-stage.sh` — the §9-step-3a-ordered runner (see Task 16)

**Interfaces:**
- Consumes: Task 14's artifact; `gate_floor` composite-key lookup; Task 6's freeze-dir assertion.
- Produces: `calibrate_two_stage pick --grid <artifact-dir>` — sweeps (rule × floor × k) from per-cell paired artifacts, applies the tie-break (higher hit@2 → lower FP → rule (i), GLM r1 minor-5), and EMITS the winning `(rule, floor, k)` + a ready-to-apply patch snippet for `WISDOM_GATE_FLOORS`/`TWO_STAGE_K`; `calibrate_two_stage flip --artifact <path>` — applies the flip ONLY if `two_stage_passes == true` (refuses otherwise, printing the letter numbers).

- [ ] **Step 1: Failing tests**: picker tie-break on synthetic grids; `flip` refuses on a failing letter AND on a missing/stale artifact; after a simulated flip, `every_non_stub_floor_key_has_a_freeze_dir` passes.
- [ ] **Step 2: Verify FAIL.**  **Step 3: Implement.**  **Step 4: Verify PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(calibration): grid picker with pinned tie-break + letter-gated flip-to-default"`

## Task 16: Step-3a ordering script — verify-scratch BEFORE calibration

**Files:**
- Create: `scripts/calibrate-two-stage.sh`
- Test: none (thin orchestration; each step's behavior is tested by its own task) — the script is validated by a shellcheck pass and a dry run in Step 4.

**Interfaces:**
- Consumes: `ct reindex verify-scratch` (Task 13), `ct reindex repair-embed-key --stale` (Task 12), `calibrate_two_stage` (Tasks 14-15).
- Produces: the ordered operational runbook the spec pins (§9 step 3/3a + §7): (1) `ct reindex verify-scratch` on the LIVE brain → stamp + sampled backfill; (2) parse coverage from its output; (3) if < 100%: `ct reindex repair-embed-key --stale`, re-verify; (4) only then run the paired calibration + pick + flip. Aborts at each gate with the spec's error-handling lines.

```bash
#!/usr/bin/env bash
set -euo pipefail
# 3a FIRST (spec §9 r22 MAJOR-4): bootstrap must precede calibration.
cargo run --manifest-path tools/Cargo.toml --bin ct -- reindex verify-scratch
cov=$(cargo run --manifest-path tools/Cargo.toml --bin ct -- reindex verify-scratch --dry-run | grep -oP 'coverage=\K[0-9.]+')
if [[ "${cov%.*}" != "100" ]]; then
  cargo run --manifest-path tools/Cargo.toml --bin ct -- reindex repair-embed-key --stale
  cargo run --manifest-path tools/Cargo.toml --bin ct -- reindex verify-scratch
fi
# Paired run (same invocation = same paired run, §7):
cargo run --manifest-path tools/Cargo.toml --bin calibrate_two_stage -- \
  --probes "$1" --baseline-key "$2" --json-out artifact.json
cargo run --manifest-path tools/Cargo.toml --bin calibrate_two_stage -- pick --grid .
cargo run --manifest-path tools/Cargo.toml --bin calibrate_two_stage -- flip --artifact artifact.json
```

- [ ] **Step 1-3:** write script; `shellcheck scripts/calibrate-two-stage.sh` clean; dry-run against a scratch Brain to verify the gate ordering refuses when coverage < 100%.
- [ ] **Step 4: Verify** (shellcheck + dry run exit codes).
- [ ] **Step 5: Commit** — `git commit -m "chore(scripts): step-3a-ordered calibration runbook (verify-scratch → repair → paired run → flip)"`

## Task 17 (conditional): Provenance backfill — ONLY if the §9 step-4 audit shows hop gaps

**Gate condition (spec §9 step 4):** expected UNNECESSARY (365/365 live). Do NOT build this task unless the post-calibration audit (`wisdom_two_stage_audit` + r28-E1 dry-run numbers) shows gap entries. If triggered, implement as its own TDD task: backfill `librarian_evidence` rows for gate-eligible entries lacking the evidence/proposal edges, using the deposit-time evidence shape (`librarian_evidence` DDL: `entry_id TEXT PRIMARY KEY REFERENCES llm_wiki_entries(id) ON DELETE CASCADE, proposal_id TEXT NOT NULL, evidence_json TEXT NOT NULL CHECK(json_valid(evidence_json)), unanchored INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL` — `src-tauri/src/db/schema.rs:392-403`); failing test asserts every gate-eligible entry is then hop-resolvable via Task 3's `candidate_set`/`member_chunk_ids` SQL (365/365-style assertion on the fixed fixture), then minimal backfill impl, then full suite. Otherwise record "step 4 not triggered" in the PR description.

---

## Explicit Non-Goals (Option A — nothing to delete, nothing to build)

- NO `ct_reindex_pass_docs` table (never existed at `bc83d06` — do-not-build, NOT a deletion).
- NO grants, no completion evaluator, no pass-aware sweep predicates, no epoch guard key purge.
- NO refusal-record table (r25-M2: no loop to suppress; the row-state reset replaces it).
- NO `IngestOutcome` enum (single typed `IngestRefused` struct only).
- NO frontend pass-progress fields; NO ops crons (BOTH DECLINED — never-dark fallback is mandatory).
- NO `embeddings`-table tagging (per-doc `documents.embed_key` is the only currency marker; spec §6).
- NO text-match chunk remap (follow-up work; mid-document-insertion test only documents today's rehash cascade).
- The unforced short-circuit at `pipeline/mod.rs:691-694` is restored AS-IS — no `embed_key` leg (§6, rev-25 m1 removed).

## Execution notes

- Subagent-driven per task (superpowers:subagent-driven-development): fresh subagent per task, sor review between tasks, fast iteration. Tasks within a group are sequential (Task 3 needs 1+2; 4 needs 3; 5 needs 4; 8 needs 7; 12/13 need 3+8; 14-15 need 5; 16 needs 12-15).
- Branch: `spec/issue271-two-stage-retrieval-impl`. Conventional commits. Regular merge only, no squash.
- Rust builds/tests SERIAL (Global Constraints): one cargo process at a time; `--test-threads=1` with `--features test-utils,mcp-server` (src-tauri) on every run.
- Step 3a (Task 16 script steps 1-3) runs on the live brain BEFORE any paired calibration — spec §9 r22 MAJOR-4.
- Tasks 6 and 15 share the freeze-dir contract: Task 6's `#[ignore]`d `replay_two_stage` is unignored only by Task 15's flip.
