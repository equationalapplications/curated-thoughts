# Forgotten-gate follow-ups Implementation Plan (#398, folds #399/#400/#401)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Carry `forgotten_at` through the OKF bundle round-trip, compute gate arms once per traversal, and add the live-half partial index.

**Architecture:** Three verticals in one PR: (1) bundle round-trip — the flag rides the log-line HTML comment grammar, applied verbatim on import behind a one-shot column probe, with guarded dup-row reconciliation; (2) traversal — a `TraversalGate` struct resolved once at EACH public walk entry (`wiki_traverse_graph` AND `wiki_context_in_scheme`) and passed down through every gate site; (3) schema — a sibling partial index mirroring the forgotten-half index's guard.

**Tech Stack:** Rust (rusqlite, src-tauri crate), TypeScript vitest (engine-parity allowlist).

**Spec:** `specs/2026-11-27-forgotten-gate-followups/spec.md` (owner-approved 2026-11-27; amended r1–r4 — see revision history; the plan argues from the amended text).

**Evidence base:** all file:line against `origin/main @ fc715239`.

**Revision history:**
- r1 (2026-11-27, Opus-medium plan review, $0.90/24 turns, REQUEST CHANGES — all findings verified, folded): B1 parser token loop dropped ids → id-first extraction; B2 round-trip fixture defeated by the survival guard → two-entry fixture; B3 new index breaks five DROP COLUMN sites → drop-list + helper; M1 export SELECT unconditional → probe + `NULL AS forgotten_at`; M2 old-binary id loss → one-way compat in spec; M3 dup-skip resurrection → UPDATE reconciliation; M4 trace-oracle uncompilable → DROP TABLE oracle + structural pass-down; minors 1–8 folded.
- r2 (2026-11-27, Opus-medium delta review, $1.02/25 turns, REQUEST CHANGES — all findings verified, folded): **B1** `wiki_context` is a SECOND public traversal (`wiki_graph.rs:2404/2418`; `CompositeWalk::walk_seed` sites `:2274/:2306/:2344` are PRODUCTION code past `mod unit_tests`' end at `:2178`, misclassified r1 as tests) → gate resolved in both public entries, `CompositeWalk` carries it; **B2** dup-UPDATE lacked the column guard → guarded, pre-v19 test extended to the dup case; **M1** integration test `ExportEvent` literal (`tests/okf_bundle.rs:137`) breaks compile and `--lib` gates are blind → literal updated + integration gate added; **M2** Step 2 promised a red that cannot happen (gate probes already shape-only on main) → expectation corrected to characterization; minors 1–5 folded.
- r3 (2026-11-27, Opus-medium delta review, $1.08/22 turns, REQUEST CHANGES — 0 BLOCKER, 4 MAJOR + 6 MINOR, all verified, folded): **M-A1** tools crate has no `[features]` table → tools gate runs plain `cargo test --lib` (no `--features test-utils`); **M-A2** the meta test never exercised the second entry's gate → added `wiki_context_in_scheme(…, Some(Scheme::Raw))` case (meta dropped → `Ok`, forgotten entries absent); **M-A3** helper ordering — `test_support.rs` + `db/mod.rs` declaration move to Task 2; **M-A4** `CompositeWalk::new(gate, truncated)` shape spelled out (`#[derive(Default)]` removed; `Default` on `TraversalGate` forbidden — an empty-arm default fails open); minors 1–6 folded (mod.rs in commit, round-trip through disk text, helper in guard test, stale `:1965` clause deleted, `load_live_node(conn, gate: &TraversalGate, …)`, dup reconciliation documented as intentionally silent).
- r4 (2026-11-27, Opus-medium delta review, $0.87/14 turns, REQUEST CHANGES — 0 BLOCKER, 1 MAJOR + 7 MINOR, all verified, folded): **M-A5** Task 5 missed the characterization test's own call sites `wiki_graph.rs:1224-1227` (E0061 once signatures change; resolution prescribed: `let gate = TraversalGate::resolve(&conn).unwrap();` then `&gate.entry_arm` args, keeping the is_none/!exists assertions — they now prove a resolved gate excludes the forgotten entry); minors 1–7 folded (`edge_vocabularies` field name, gate origin at `:2110`, three stale Task-6 mentions, `open_in_memory()` fixtures prescribed — `engine9_migrated_fixture` is private to connection.rs tests, `event_line.rs:65/:107` callers named, `test_support` sibling noted, revision-header staleness). Reviewer prescribed a TEXT-ONLY confirm for r5 rather than a full round.

## Global Constraints

- One canonical build location: `export CARGO_TARGET_DIR=/home/kv-thinkpad-t420-ubuntu/code/github/equationalapplications/curated-thoughts/.cargo-shared-target` (ABSOLUTE — repo AGENTS.md). Pre-flight `df -h` ≥30G free.
- Gate semantics frozen: fail-closed scoring-path contract, survival-guard predicate, carve-outs unchanged (spec Non-goals).
- `temporal_columns_shape_only` is the ONLY probe the gate uses; gate reads never touch `llm_wiki_meta`.
- Owner note 1 (Kurt 2026-11-27): parse `forgotten:` strictly inside the trailing `<!-- … -->` comment; summary text containing `forgotten:` is never parsed; existing id-only lines byte-identical.
- Owner note 2 (Kurt 2026-11-27): EVERY public traversal entry point passes walk-start arms down. There are TWO (r2-B1): `wiki_traverse_graph` (`wiki_graph.rs:779`) and `wiki_context_in_scheme` (`:2418`; `wiki_context` `:2404` delegates to it). Both resolve the gate once at entry.
- **DROP COLUMN + partial index (r1-B3):** SQLite refuses `ALTER TABLE … DROP COLUMN` while a partial index's WHERE clause uses the column. The live index adds a THIRD index to drop at every pre-v19-shape test fixture: `connection.rs:8212-8214`, `entities.rs:1306-1308`, `events.rs:231-233`, `wisdom_match.rs:1948-1950`, `wisdom_match.rs:2133-2135` (r3 minor 4: the `:1965` sibling drops `related_entry_id`, not `forgotten_at` — no change needed there). ONE shared test helper `drop_forgotten_column(conn)` (drops all three indexes, then the column) lives in `src-tauri/src/db/test_support.rs` (r2 minor 3: a named module) and is created in TASK 2 (r3 M-A3 — Tasks 2 and 4 use it before Task 6 exists; the `DROP INDEX IF EXISTS` lines make it correct before the live index exists).
- No new issues from this lane — findings fold into PR #402.
- Spec's `Errata` section is the durable home for EQP measurements (plan.md/LEDGER.md are deleted at cleanup).

## File Structure

- `src-tauri/src/okf/log_md.rs` — comment grammar (write + parse). Task 1.
- `src-tauri/src/okf/event_line.rs` — `build_event_text` signature + tests. Task 2.
- `src-tauri/src/okf/bundle_write.rs` — `ExportEvent` field, log-entry build. Task 2.
- `src-tauri/src/db/bundle_io.rs` — export SELECT + shape probe. Task 2.
- `src-tauri/tests/okf_bundle.rs` — `ExportEvent` literal (integration target). Task 2.
- `src-tauri/src/okf/bundle_read.rs` — `ParsedEvent` field + populate. Task 3.
- `src-tauri/src/db/bundle_apply.rs` — insert + shape probe + dup reconciliation + round-trip test. Task 4.
- `src-tauri/src/wisdom_match.rs` — `entry_not_forgotten_arm_for_temporal` helper. Task 5.
- `src-tauri/src/wiki_graph.rs` — `TraversalGate` + pass-down (both public entries + CompositeWalk) + tests. Task 5.
- `src-tauri/src/db/okf_ddl.rs` — live-half index. Task 6.
- `src-tauri/src/db/connection.rs` — guard test + expected-list row. Task 6.
- `src-tauri/src/db/test_support.rs` (new, created in TASK 2 — r3 M-A3) — `drop_forgotten_column` helper (r3 minor 6: name collides only as a sibling with `crate::db::proposals::test_support`; paths disjoint, noted for grep).
- `src/__tests__/engine9-setup-compat.test.ts` — CT_OWNED allowlist row. Task 6.

---

### Task 1: log-line comment grammar — `forgotten:` key (log_md.rs)

**Files:**
- Modify: `src-tauri/src/okf/log_md.rs:33-70`
- Test: `src-tauri/src/okf/log_md.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Produces: `pub fn append_event_id_comment_forgotten(text: &str, event_id: &str, forgotten_at: Option<i64>) -> String`; `parse_event_id_comment` return type becomes `(String, Option<String>, Option<i64>)` — `(stripped_text, event_id, forgotten_at)`. All callers updated in Tasks 2–3.

Grammar: `({event_type}) {summary} <!-- id: evt_x forgotten:1732713600000 -->` — the key is `forgotten:` with NO space, digits only, inside the trailing comment. The parser already keys on the LAST `<!--` in the line, which satisfies the owner's strict-scoping note: `forgotten:` occurrences in summary text are never seen because only the trailing comment is inspected.

- [ ] **Step 1: Write the failing tests** (in log_md.rs test module)

```rust
#[test]
fn append_writes_forgotten_key_when_present() {
    let t = append_event_id_comment_forgotten("(note) hi", "evt_x", Some(1732713600000));
    assert_eq!(t, "(note) hi <!-- id: evt_x forgotten:1732713600000 -->");
}

#[test]
fn append_delegates_to_id_only_when_absent() {
    let t = append_event_id_comment_forgotten("(note) hi", "evt_x", None);
    assert_eq!(t, "(note) hi <!-- id: evt_x -->");
}

#[test]
fn append_invalid_id_writes_no_comment_even_with_forgotten() {
    // r1 minor 3: the id-validation guard applies on BOTH paths.
    let t = append_event_id_comment_forgotten("(note) hi", "bad id!", Some(1000));
    assert_eq!(t, "(note) hi");
}

#[test]
fn parse_reads_forgotten_from_trailing_comment_only() {
    // `forgotten:` in the summary must NOT be parsed (owner note 1).
    let (text, id, forgotten) = parse_event_id_comment(
        "(note) forgotten:999 <!-- id: evt_x forgotten:1732713600000 -->",
    );
    assert_eq!(text, "(note) forgotten:999");
    assert_eq!(id.as_deref(), Some("evt_x"));
    assert_eq!(forgotten, Some(1732713600000));
}

#[test]
fn parse_id_only_line_yields_none_and_identical_text() {
    let line = "(action) did a thing <!-- id: evt_9 -->";
    let (text, id, forgotten) = parse_event_id_comment(line);
    assert_eq!(text, "(action) did a thing");
    assert_eq!(id.as_deref(), Some("evt_9"));
    assert_eq!(forgotten, None);
}

#[test]
fn parse_non_numeric_or_negative_forgotten_parses_as_absent() {
    // r1 minor 2: "-5".parse::<i64>() succeeds — filter negatives out to
    // match the writer's ms >= 0 guard.
    for bad in ["notanumber", "-5"] {
        let line = format!("(note) hi <!-- id: evt_x forgotten:{bad} -->");
        let (text, id, forgotten) = parse_event_id_comment(&line);
        assert_eq!(id.as_deref(), Some("evt_x"));
        assert_eq!(forgotten, None);
        assert_eq!(text, "(note) hi");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `CARGO_TARGET_DIR=<repo>/.cargo-shared-target cargo test --manifest-path src-tauri/Cargo.toml --lib okf::log_md`
Expected: FAIL — `append_event_id_comment_forgotten` undefined; parse arity mismatch.

- [ ] **Step 3: Implement**

`append_event_id_comment_forgotten` — the id-validation guard applies on BOTH paths (r1 minor 3):

```rust
pub fn append_event_id_comment_forgotten(
    text: &str,
    event_id: &str,
    forgotten_at: Option<i64>,
) -> String {
    let valid = !event_id.is_empty()
        && event_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-');
    match (valid, forgotten_at) {
        (true, Some(ms)) if ms >= 0 => format!("{text} <!-- id: {event_id} forgotten:{ms} -->"),
        (true, _) => append_event_id_comment(text, event_id),
        (false, _) => text.to_string(),
    }
}
```

`parse_event_id_comment` — keep the existing last-`<!--` discovery and the overall invalid-id behavior EXACTLY (byte-identical stripped text; an invalid or missing id → `(stripped, None, None)`). Change ONLY how the id and the new key are read out of `inner`. r1-B1: do NOT use a bare whitespace token loop — `"id:"` alone would strip to an empty id, and `.all()` on an empty string passes, so every import would get `id = ''` and `INSERT OR IGNORE` would silently drop all but the first event. Keep today's `strip_prefix("id:")` + trim, split the id off the front of the remainder, then scan only the tail for the key:

```rust
let mut event_id: Option<String> = None;
let mut forgotten_at: Option<i64> = None;
if let Some(rest) = inner.strip_prefix("id:") {
    let rest = rest.trim();
    let (id_tok, tail) = match rest.split_once(char::is_whitespace) {
        Some((a, b)) => (a, b),
        None => (rest, ""),
    };
    if !id_tok.is_empty()
        && id_tok
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        event_id = Some(id_tok.to_string());
        for tok in tail.split_whitespace() {
            if let Some(v) = tok.strip_prefix("forgotten:") {
                forgotten_at = v.parse::<i64>().ok().filter(|ms| *ms >= 0);
            }
        }
    }
}
```

Return `(stripped, event_id, forgotten_at)`. Behavior note (r2 minor 1 — stated precisely, not as "identical"): every WELL-FORMED line the old parser accepted parses identically (same stripped text, same id). Three edge behaviors change, all improvements — document them in the fn doc comment: an empty id (`<!-- id: -->`) and a bare `note id:` prefix now yield `None` (old code yielded `Some("")`, which would insert `id = ''`); a trailing junk token (`<!-- id: evt_x junk -->`) now yields `Some("evt_x")` (old code rejected the whole comment).

- [ ] **Step 4: Fix the in-file caller(s) and run to green**

`parse_event_id_comment`'s other caller is `event_line.rs:29` (`parse_event_text`) — update its destructure to the 3-tuple there (Task 3 finishes the plumbing; here just destructure and ignore: `let (without_id, event_id, _forgotten) = …`). Run the Task 1 tests: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/okf/log_md.rs src-tauri/src/okf/event_line.rs
git commit -m "feat(okf): forgotten: key in the event-line id comment grammar (#398)"
```

### Task 2: export side — ExportEvent carries the flag (bundle_io.rs, bundle_write.rs, event_line.rs, tests/okf_bundle.rs)

**Files:**
- Modify: `src-tauri/src/okf/bundle_write.rs:18-29` (struct), `:159-176` (log build)
- Modify: `src-tauri/src/db/bundle_io.rs` (`load_export_entities` `:11` probe site; `load_events` `:222-241` SELECT)
- Modify: `src-tauri/tests/okf_bundle.rs:137-143` (`ExportEvent` literal — r2-M1)
- Modify: `src-tauri/src/okf/event_line.rs:11-27` (build_event_text), `:60-110` (tests)
- Create: `src-tauri/src/db/test_support.rs` + `#[cfg(test)] pub(crate) mod test_support;` in `src-tauri/src/db/mod.rs` (r3 M-A3 + minor 1 — Task 2's own test needs the helper, so the helper lands HERE, not Task 6)

**Interfaces:**
- Consumes: `append_event_id_comment_forgotten` (Task 1).
- Produces: `ExportEvent { …, pub forgotten_at: Option<i64> }`; `build_event_text(event_type: &str, summary: &str, related_path: Option<&str>, event_id: &str, forgotten_at: Option<i64>) -> String`.

- [ ] **Step 1: Write the failing tests**

event_line.rs (signature):

```rust
#[test]
fn build_event_text_carries_forgotten_key() {
    let text = build_event_text("action", "did a thing", None, "evt_x", Some(1000));
    assert_eq!(text, "(action) did a thing <!-- id: evt_x forgotten:1000 -->");
    let text = build_event_text("action", "did a thing", None, "evt_x", None);
    assert_eq!(text, "(action) did a thing <!-- id: evt_x -->");
}
```

bundle_io.rs (r1-M1 — pre-v19 export tolerance; fixture: r4 minor 4 — `engine9_migrated_fixture` is PRIVATE to connection.rs's test module, so use `open_in_memory()` here, which already yields the v19 shape):

```rust
#[test]
fn export_tolerates_pre_v19_shape_without_forgotten_column() {
    // open_in_memory() fixture (v19 shape), then drop the column via the
    // drop_forgotten_column helper (created THIS task, r3 M-A3 — all
    // three indexes first). Export must succeed; events export with
    // forgotten_at == None.
}
```

- [ ] **Step 2: Run to verify failure** (one filter per invocation — r1 minor 4)

Run: `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib okf::event_line`
Then: `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib bundle_io`
Expected: FAIL (arity; probe/test missing).

- [ ] **Step 3: Implement**

`event_line.rs`: add the `forgotten_at: Option<i64>` param; final line becomes
`append_event_id_comment_forgotten(&core, event_id, forgotten_at)`.
Both existing test callers of `build_event_text` (r4 minor 5 — `event_line.rs:65` and `:107`) gain `None` as the fifth argument.
`bundle_write.rs:18-29`: add `pub forgotten_at: Option<i64>,` to `ExportEvent` (doc comment: `/// Engine `forget()` tombstone, ms epoch; `None` = live. Rides the id comment on export (#398).`).
`bundle_write.rs:172` (`build_event_text(...)` call in the log_entries map): pass `ev.forgotten_at`.
bundle_io.rs (r1-M1, site named per r2 minor 4): probe ONCE in `load_export_entities` (`bundle_io.rs:11`) BEFORE the per-cluster loop (`:55` calls `load_events` per cluster — the probe must NOT sit inside it) via `crate::db::ddl_compat::existing_columns(conn, "llm_wiki_events")`, and pass `has_forgotten: bool` into `load_events`; the SELECT appends `, forgotten_at` when present, `, NULL AS forgotten_at` when not (so `let forgotten_at: Option<i64> = r.get(5)?;` works for both shapes). A pre-v19 brain exports without error — matching the codebase's degraded-shape tolerance everywhere else (spec r1-M1 ruling).
`tests/okf_bundle.rs:137-143` (r2-M1): the `ExportEvent { … }` literal gains `forgotten_at: ev.forgotten_at` — this is a `ParsedEvent → ExportEvent` round-trip helper, so pass the flag through (no E0063, no silent drop).
`db/test_support.rs` (r3 M-A3 + minor 1): create NOW (Tasks 2/4 need it; Task 6 only converts sites) with `drop_forgotten_column`, and declare `#[cfg(test)] pub(crate) mod test_support;` in `src-tauri/src/db/mod.rs` (submodules declared at `mod.rs:1-43`; follow the existing `pub mod x;` pattern):

```rust
//! Test-only DB shape helpers shared across db test modules.
//! Drop the three forgotten_at partial indexes, then the column — the
//! exact order SQLite requires before ALTER TABLE … DROP COLUMN (r1-B3).
//! `DROP INDEX IF EXISTS` keeps this correct before the live index
//! exists (Task 6 adds it later).
#[cfg(test)]
pub(crate) fn drop_forgotten_column(conn: &rusqlite::Connection) {
    conn.execute_batch(
        "DROP INDEX IF EXISTS llm_wiki_events_related_live_idx;
         DROP INDEX IF EXISTS llm_wiki_events_related_forgotten_idx;
         DROP INDEX IF EXISTS llm_wiki_events_forgotten_idx;
         ALTER TABLE llm_wiki_events DROP COLUMN forgotten_at;",
    ).unwrap();
}
```

- [ ] **Step 4: Run to green**

Run: `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib okf::`
Then: `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib bundle_io`
Then: `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --test okf_bundle` (r2-M1: the integration target compiles + goldens pass).
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/okf/event_line.rs src-tauri/src/okf/bundle_write.rs src-tauri/src/db/bundle_io.rs src-tauri/tests/okf_bundle.rs src-tauri/src/db/test_support.rs src-tauri/src/db/mod.rs
git commit -m "feat(bundle): export carries forgotten_at in ExportEvent + log-line grammar (#398)"
```

### Task 3: parse side — ParsedEvent carries the flag (bundle_read.rs)

**Files:**
- Modify: `src-tauri/src/okf/bundle_read.rs:18-25` (struct), `:245-273` (populate)

**Interfaces:**
- Consumes: Task 1's 3-tuple `parse_event_id_comment`.
- Produces: `ParsedEvent { …, pub forgotten_at: Option<i64> }`.

- [ ] **Step 1: Write the failing test** (bundle_read.rs test module — copy the module's existing bundle-dir fixture setup verbatim; only the two log lines and the assertions are new)

```rust
#[test]
fn parse_populates_forgotten_from_comment() {
    // Entity log carries exactly two event lines:
    //   "(action) did a thing <!-- id: evt_a forgotten:1732713600000 -->"
    //   "(action) did another thing <!-- id: evt_b -->"
    // assert: events[0].forgotten_at == Some(1732713600000)
    //         events[1].forgotten_at == None
}
```

- [ ] **Step 2: Run to verify failure** — `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib okf::bundle_read`. Expected: FAIL (field missing).

- [ ] **Step 3: Implement**

`bundle_read.rs:18-25`: add `pub forgotten_at: Option<i64>,` to `ParsedEvent`.
`bundle_read.rs:245-273`: `parse_event_text`'s destructure gains the third slot; `entity.events.push(ParsedEvent { …, forgotten_at })`.

- [ ] **Step 4: Run to green** — `cargo test --manifest-path src-tauri/Cargo.toml --lib okf::` PASS. Any other `ParsedEvent` constructions live in bundle_apply.rs tests (`:1137`) — they gain `forgotten_at: None` in Task 4.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/okf/bundle_read.rs
git commit -m "feat(bundle): ParsedEvent carries forgotten_at from the id comment (#398)"
```

### Task 4: import side — apply verbatim behind a one-shot shape probe, with guarded dup reconciliation (bundle_apply.rs)

**Files:**
- Modify: `src-tauri/src/db/bundle_apply.rs:371-385` (probe), `:712-753` (insert + dup path), `:1137` (test fixture field)

**Interfaces:**
- Consumes: `ParsedEvent.forgotten_at` (Task 3); `crate::db::ddl_compat::existing_columns`.

- [ ] **Step 1: Write the failing tests** (#398 acceptance, bundle_apply.rs tests)

```rust
#[test]
fn round_trip_preserves_forgotten_at_and_gate_hides_entry() {
    // r1-B2 redesign — the survival guard keeps an entry alive if ANY live
    // backing event exists (forgotten_entry_arm's `OR EXISTS … IS NULL`),
    // so the two events must back DIFFERENT entries:
    //
    // 1. Source brain (r4 minor 4: `engine9_migrated_fixture` is private
    //    to connection.rs's test module — use `open_in_memory()`, which
    //    already yields the v19 shape): seed TWO live entries `fact_a` and
    //    `fact_b`, both source_type NOT 'user_stated' (carve-out!) and
    //    neither id == entity_id nor tier_* (carve-outs!). Events:
    //    evt_dead (related_entry_id='fact_a', forgotten_at set via the
    //    engine-shape UPDATE) and evt_live (related_entry_id='fact_b',
    //    forgotten_at NULL).
    // 2. Round-trip through DISK TEXT (r3 minor 2 — skipping this drops
    //    the Task 1-3 grammar from the #398 acceptance): bundle_io
    //    export → bundle_write to a temp dir → bundle_read parse the
    //    files back → apply_import(&mut conn, &bundle, ImportMode::Merge)
    //    on the FRESH temp brain. Never build the ParsedBundle in memory.
    // 4. Assert (format! the arm in — no pseudo-SQL):
    //      let arm = crate::wisdom_match::forgotten_entry_arm(
    //          &crate::wisdom_match::temporal_columns_shape_only(&conn)?);
    //      let sql = format!("SELECT COUNT(*) FROM llm_wiki_entries e
    //          WHERE e.id='fact_a' AND e.deleted_at IS NULL{arm}");
    //      → 0 on the importing brain (fact_a hidden);
    //      the same query for fact_b → 1 (fact_b visible);
    //      imported evt_dead row carries forgotten_at; evt_live IS NULL.
}

#[test]
fn dup_import_reconciles_a_forgotten_event() {
    // r1-M3: brain B already has evt_dead LIVE (imported before the
    // forget). A later bundle says evt_dead forgotten_at = Some(ms).
    // event_exists marks it dup — but the import must still UPDATE
    //   SET forgotten_at = ? WHERE id = ? AND forgotten_at IS NULL
    // so an already-synced brain cannot keep the resurrection alive.
    // Assert: events_skipped unchanged, row's forgotten_at now set.
}

#[test]
fn pre_v19_target_imports_without_forgotten_column() {
    // Fresh fixture; drop the column via the drop_forgotten_column helper
    // (r1-B3: all three indexes go first). Apply a bundle whose event
    // carries Some(ms) — assert: no error, events_added == 1.
    // r2-B2 extension: apply the SAME bundle a SECOND time (now a dup —
    // event_exists matches by id) with forgotten_at still Some(ms).
    // Assert: no "no such column" abort, no rollback, events_skipped == 1,
    // import succeeds.
}

#[test]
fn legacy_bundle_without_key_imports_live() {
    // apply_import a ParsedBundle whose ParsedEvent.forgotten_at == None
    // onto a v19 brain — assert row's forgotten_at IS NULL.
}
```

- [ ] **Step 2: Run to verify failure** — `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib bundle_apply`. Expected: FAIL (round-trip loses the flag; no reconciliation).

- [ ] **Step 3: Implement**

In `apply_import` (`:371`), after `ImmediateTx::begin` (:384), probe ONCE:

```rust
// #398: one column probe per import, not per event. Pre-v19 targets
// (no forgotten_at) import without the column — unchanged behavior.
let events_have_forgotten_at = crate::db::ddl_compat::existing_columns(
    &tx, "llm_wiki_events",
)?.iter().any(|c| c == "forgotten_at");
```

In the event loop (:712-753), keep the dup decision exactly as-is (:714-733), then reconcile a dup that arrived forgotten — GUARDED by the same probe (r2-B2: an unguarded `UPDATE … forgotten_at` on a pre-v19 target fails prepare with "no such column" and the `?` rolls back the whole import — a regression the spec forbids):

```rust
if is_dup {
    if events_have_forgotten_at {
        if let (Some(ms), Some(id)) = (ev.forgotten_at, ev.event_id.as_deref()) {
            // #398: a dup whose source row is forgotten must tombstone the
            // target's live row — otherwise an already-synced brain keeps
            // the forgotten event alive forever (the exact resurrection
            // #398 fixes). Pre-v19 targets skip this (no column to set).
            tx.execute(
                "UPDATE llm_wiki_events SET forgotten_at = ?1
                 WHERE id = ?2 AND forgotten_at IS NULL",
                params![ms, id],
            )?;
        }
    }
    result.events_skipped += 1;
    continue;
}
```

For the non-dup insert, branch on the probe (static SQL — a column that may not exist cannot be named):

```rust
let related = ev.related_entry_id.as_deref().map(|id| mapped(id, &id_map));
if events_have_forgotten_at {
    tx.execute(
        "INSERT OR IGNORE INTO llm_wiki_events
            (id, entity_id, event_type, summary, related_entry_id, created_at, forgotten_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![event_id, target_entity_id, ev.event_type, ev.summary,
                related, created_at, ev.forgotten_at],
    )?;
} else {
    tx.execute(
        "INSERT OR IGNORE INTO llm_wiki_events
            (id, entity_id, event_type, summary, related_entry_id, created_at)
         VALUES (?1,?2,?3,?4,?5,?6)",
        params![event_id, target_entity_id, ev.event_type, ev.summary,
                related, created_at],
    )?;
}
```

Semantics note (spec r1-M3 ruling): tombstone wins on conflict — a target row already forgotten is never un-forgotten by an incoming live event; only a live target row is tombstoned by an incoming forgotten event. Reporting note (r3 minor 6): a reconciled dup counts only as `events_skipped` — the tombstone is INTENTIONALLY silent in `ImportResult` (documented here and in the code comment; no new counter). Update the `:1137` test fixture's `ParsedEvent` construction with `forgotten_at: None`.

- [ ] **Step 4: Run to green** — `cargo test --manifest-path src-tauri/Cargo.toml --lib bundle_apply` PASS (all four new tests + existing).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/db/bundle_apply.rs
git commit -m "feat(bundle): import applies forgotten_at verbatim, reconciles dups (#398)"
```

### Task 5: gate arms once per traversal, both public entries (wisdom_match.rs, wiki_graph.rs)

**Files:**
- Modify: `src-tauri/src/wisdom_match.rs:675-700` (`entry_not_forgotten_arm_for` split)
- Modify: `src-tauri/src/wiki_graph.rs` — `load_live_entry` (:466-490), `load_live_node` (:527-541), `fetch_neighbors` + outbound/inbound (:541-707), `wiki_traverse_graph` (:779) + private walkers (:799-1010), `live_entry_exists` (:1150-1162), characterization-test call sites :1224-1227 (r4 M-A5), **production composite walk** (`CompositeWalk` :2235, `walk_seed` :2261 with PRODUCTION sites :2274/:2306/:2344, `wiki_context` :2404, `wiki_context_in_scheme` :2418), test call site :2110-2111 (r2-B1)

**Interfaces:**
- Produces (wisdom_match.rs): `pub(crate) fn entry_not_forgotten_arm_for_temporal(t: &Temporal, alias: &str) -> Result<String>` — the alias-validation + rewrite half of `entry_not_forgotten_arm_for`, factored verbatim.
- Produces (wiki_graph.rs): `struct TraversalGate { entry_arm: String, arm_s: String, arm_t: String }` + `fn resolve(conn: &Connection) -> Result<TraversalGate>`.

- [ ] **Step 1: Write the failing tests** (wiki_graph.rs tests)

```rust
#[test]
fn gate_probe_never_touches_llm_wiki_meta() {
    // Closes #399 item 1's verification claim. r1 minor 6: DROP the meta
    // table entirely — a corrupt VALUE only proves the gate doesn't error;
    // a missing TABLE proves the gate never reads it (any read would error
    // "no such table"). Key name wisdom_active_scheme confirmed at
    // embed_scheme.rs:67.
    //
    // r2-M2 expectation, stated honestly: the gate probes are ALREADY
    // shape-only on main (4ac2ca12) — this test is a CHARACTERIZATION that
    // passes before AND after; the only red driving this task is the
    // TraversalGate compile failure. The test pins the invariant against
    // regression; it does not prove the hoist.
    //
    // Coverage (r2-B1 + r3 M-A2), with llm_wiki_meta dropped on a v19
    // fixture:
    //   - wiki_traverse_graph (entry-space seed) → Ok, forgotten entries
    //     excluded; live_entry_exists → Ok (gate arms inert of meta);
    //   - wiki_context → ERRORS fail-closed: it resolves reader_scheme at
    //     :2411 for SCORING — a deliberate meta read OUTSIDE the gate;
    //     failing closed on a missing meta table is correct and unchanged;
    //   - wiki_context_in_scheme(&conn, …, Some(Scheme::Raw)) → Ok with
    //     the gate LIVE (r3 M-A2: caller-resolved scheme bypasses the
    //     meta read, so this is the only case that exercises the second
    //     entry's gate with meta gone). Assert: forgotten entries absent
    //     from the result's entities, survival-guard entries present. A
    //     regression that reads meta inside the CompositeWalk gate path
    //     fails this case.
}

#[test]
fn traversal_gate_resolves_both_shapes() {
    // Unit test on TraversalGate::resolve: v19 fixture → all three arms
    // non-empty; pre-v19 fixture (drop_forgotten_column helper) → all
    // three arms EMPTY (gate inert, behavior identical to today).
    //
    // r1-M4: the once-per-walk property is enforced STRUCTURALLY, not by a
    // trace counter — every gate consumer takes &TraversalGate or &str arm
    // params, so no per-node probe path exists to call (rusqlite's `trace`
    // feature is not enabled — workspace Cargo.toml:30 — and
    // `Connection::trace` takes a fn pointer; oracle declined).
}
```

Existing characterization tests must stay green:
`forgotten_gate_closes_graph_bypass_sites`, `forgotten_neighbor_edges_are_gated_in_traversal`.

- [ ] **Step 2: Run to verify failure** — `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib wiki_graph`. Expected (r2-M2): the meta test is a characterization — it PASSES pre-change; the RED is the `traversal_gate_resolves_both_shapes` compile failure (TraversalGate doesn't exist) plus the new signatures. If the meta test FAILS pre-change, STOP and investigate — something on the walk path reads meta that the r2 evidence pass missed (e.g. ontology or edge-vocabulary resolution); report before proceeding.

- [ ] **Step 3: Implement**

wisdom_match.rs — split, no behavior change:

```rust
/// Alias-parameterized arm derived from an ALREADY-RESOLVED Temporal.
/// `entry_not_forgotten_arm_for` = shape probe + this. Traversal (issue
/// #400) resolves the shape once per walk and calls this per alias.
pub(crate) fn entry_not_forgotten_arm_for_temporal(
    t: &Temporal, alias: &str,
) -> Result<String> {
    // (move the alias-ensure! guards and the rewrite block here verbatim)
}
```

`entry_not_forgotten_arm_for` becomes: shape probe → call the new fn. (Keep its doc; it stays the tools-crate entry.)

wiki_graph.rs:

```rust
/// Walk-start gate arms (issue #400): resolved ONCE at each PUBLIC
/// traversal entry and passed down. A mid-walk schema change observes the
/// walk-start shape — accepted (spec §Task 3, amended r2). Owner note 2:
/// there are TWO public entries (r2-B1) — `wiki_traverse_graph` (:779)
/// and `wiki_context_in_scheme` (:2418; `wiki_context` :2404 delegates to
/// it). Both resolve the gate before their walk and pass it down. The
/// entity-space path (`fetch_entity_neighbors`) does NOT gate: it joins
/// `curated_entities`, which has no `forgotten_at`.
struct TraversalGate { entry_arm: String, arm_s: String, arm_t: String }

impl TraversalGate {
    fn resolve(conn: &Connection) -> Result<Self> {
        let t = crate::wisdom_match::temporal_columns_shape_only(conn)?;
        let entry_arm = crate::wisdom_match::forgotten_entry_arm(&t);
        Ok(Self {
            arm_s: crate::wisdom_match::entry_not_forgotten_arm_for_temporal(&t, "s")?,
            arm_t: crate::wisdom_match::entry_not_forgotten_arm_for_temporal(&t, "t")?,
            entry_arm,
        })
    }
}
```

Pass-down for the FIRST entry — `wiki_traverse_graph`:
- `let gate = TraversalGate::resolve(conn)?;` first line; pass `&gate` to both private walkers.
- `scoped_traverse` / `cross_partition_traverse`: take `gate: &TraversalGate`; pass into `load_live_node`, `load_live_entry` (param `arm: &str` → uses `gate.entry_arm`), `fetch_neighbors`, `live_entry_exists`.
- `fetch_neighbors` → `fetch_outbound_neighbors` / `fetch_inbound_neighbors`: take `gate: &TraversalGate`, replace the per-call `entry_not_forgotten_arm_for` with `gate.arm_s` / `gate.arm_t`.
- `live_entry_exists` (:1150): take `arm: &str`, drop its probe.

Pass-down for the SECOND entry — `wiki_context_in_scheme` (r2-B1):
- `let gate = TraversalGate::resolve(conn)?;` BEFORE the seed loop (up to 25 seeds), then hand it to `CompositeWalk` — r3 M-A4 shape, exact:
  - REMOVE `#[derive(Default)]` from `CompositeWalk` (`:2234`) — a `gate: TraversalGate` field has no Default, and deriving Default on `TraversalGate` is FORBIDDEN: empty arms mean an inert gate, i.e. the walk FAILS OPEN on a v19 brain. Put that reasoning in a comment on `TraversalGate` and add `impl` nothing — no Default impl for it.
  - Add `fn new(gate: TraversalGate, truncated: bool) -> Self` initializing EVERY existing field (`:2236-2247` — r4 minor 1: the vocab field is named `edge_vocabularies`, not `per_seed_vocabs`) plus `gate`.
  - Production site `:2433-2447` (the struct literal with `truncated: clamped_depth < depth` + `..Default::default()`): replace with `CompositeWalk::new(gate, clamped_depth < depth)`.
  - Test site `:2110` (`CompositeWalk::default()`): replace with `let gate = TraversalGate::resolve(&conn).unwrap(); CompositeWalk::new(gate, false)` (r4 minor 2 — the gate's origin is written out) — note `:2111` is the `walk_seed` CALL; the construction is `:2110` (r3 M-A4 citation fix).
  - Characterization-test call sites `:1224-1227` (r4 M-A5 — `forgotten_gate_closes_graph_bypass_sites` calls both functions directly and must compile + stay green): `let gate = TraversalGate::resolve(&conn).unwrap();` then `load_live_entry(&conn, "ent", "gone", &gate.entry_arm)` / `live_entry_exists(&conn, "gone", &gate.entry_arm)`, same for `"kept"`. KEEP the `is_none` / `!exists` assertions — with the arm parameter they now prove a RESOLVED gate excludes the forgotten entry (an empty arm would let `"gone"` through).
- `walk_seed` (:2261) uses `self.gate`: `:2274` `load_live_node(&conn, &self.gate, …)` (r3 minor 5 — DECIDED signature: `load_live_node(conn, gate: &TraversalGate, …)`, which passes `&gate.entry_arm` to `load_live_entry`; applied at BOTH callers `:808` and `:2274`), `:2306` `fetch_neighbors(..., &self.gate, ...)`, `:2344` `load_live_entry(..., &self.gate.entry_arm)`.
- Reclassification (r2-B1): `:2274/:2306/:2344` are PRODUCTION sites, NOT test call sites. The genuine test call site is `:2110-2111` (`CompositeWalk::new` + `walk.walk_seed`).
- `wiki_context` (:2404) needs no separate resolve — it delegates to `wiki_context_in_scheme`.
- `scoped_traverse`'s `load_live_node` call (`:808`) gets the same `&TraversalGate` signature.

- [ ] **Step 4: Run to green** — `cargo test --manifest-path src-tauri/Cargo.toml --lib wiki_graph` PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/wisdom_match.rs src-tauri/src/wiki_graph.rs
git commit -m "perf(traversal): gate arms resolved once per walk in both public entries (#400, closes #399)"
```

### Task 6: live-half partial index (#401)

**Files:**
- Modify: `src-tauri/src/db/okf_ddl.rs:379-401`
- Modify: `src-tauri/src/db/connection.rs:8186-8215` (expected list + guard test)
- Modify: the five pre-v19-shape test sites (see Global Constraints) to use the `drop_forgotten_column` helper (created in Task 2 — r3 M-A3)
- Modify: `src/__tests__/engine9-setup-compat.test.ts:247-256` (CT_OWNED set)

**Interfaces:** none consumed; produces index `llm_wiki_events_related_live_idx`.

- [ ] **Step 1: Write the failing tests**

connection.rs (mirror `related_forgotten_idx_is_guarded_on_both_columns`, `:8200`, in the SAME style — copy the forgotten-index test's post-DROP re-apply block verbatim, substituting the index name, and drop ALL THREE indexes before the column):

```rust
/// #401: the survival guard's LIVE half (`forgotten_at IS NULL`) is served
/// by its own partial index, gated on both columns like the forgotten idx.
#[test]
fn related_live_idx_is_guarded_on_both_columns() {
    let idx_count = |c: &Connection| -> i64 {
        c.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='llm_wiki_events_related_live_idx'",
            [], |r| r.get(0),
        ).unwrap()
    };
    let (_tmp, conn) = engine9_migrated_fixture();
    assert_eq!(idx_count(&conn), 1, "v19 shape carries the index");
    // r1-B3: all three partial indexes must go before the column drop,
    // or SQLite refuses ALTER TABLE … DROP COLUMN.
    conn.execute_batch(
        "DROP INDEX llm_wiki_events_related_live_idx;
         DROP INDEX llm_wiki_events_related_forgotten_idx;
         DROP INDEX llm_wiki_events_forgotten_idx;
         ALTER TABLE llm_wiki_events DROP COLUMN forgotten_at;",
    ).unwrap();
    // …then the forgotten-index test's re-apply block, verbatim (:8216+):
    // re-run apply_llm_wiki_v18_v19_partial_indexes on the degraded shape,
    // assert idx_count == 0 (no error, no index).
}
```

engine9-setup-compat.test.ts — add to `CT_OWNED`:

```typescript
"index:llm_wiki_events_related_live_idx",
```

Add `("index", "llm_wiki_events_related_live_idx")` to the Rust expected list (r2 minor 5: after `("index", "llm_wiki_events_related_forgotten_idx")` at `:8193`, BEFORE the trigger row at `:8194`).

The shared helper already exists (`db/test_support.rs`, created in Task 2 — r3 M-A3). Use it HERE too (r3 minor 3 — one source of truth for the drop order): the guard test above replaces its inline `DROP INDEX` batch with `crate::db::test_support::drop_forgotten_column(&conn);` followed by the column-drop assertion flow — keep the re-apply block verbatim. Convert all five sites to the helper: `connection.rs:8212-8214` (the forgotten guard test), `entities.rs:1306-1308`, `events.rs:231-233`, `wisdom_match.rs:1948-1950`, `wisdom_match.rs:2133-2135`.

- [ ] **Step 2: Run to verify failure** — `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib related_live_idx`. Expected: FAIL (index not created).

- [ ] **Step 3: Implement** (okf_ddl.rs, inside the existing `related_entry_id` guard, after the forgotten idx):

```rust
// Issue #401: the survival guard's live half (`le.forgotten_at IS NULL`)
// has no serving index — the forgotten idx's rows are all forgotten rows,
// so its WHERE clause cannot satisfy IS NULL. Both-column guard mirrors
// the arm's probe guard (forgotten_entry_arm).
conn.execute(
    "CREATE INDEX IF NOT EXISTS llm_wiki_events_related_live_idx ON llm_wiki_events(related_entry_id) WHERE forgotten_at IS NULL AND related_entry_id IS NOT NULL",
    [],
)?;
```

- [ ] **Step 4: EQP verification + Errata (NOT a unit test)**

r2 minor 2 — run EQP on the REAL correlated query (a literal probe is not what the index must serve; partial-index implication differs for correlated terms):
`EXPLAIN QUERY PLAN SELECT e.id FROM llm_wiki_entries e WHERE e.deleted_at IS NULL AND (NOT EXISTS (SELECT 1 FROM llm_wiki_events fe WHERE fe.related_entry_id = e.id AND fe.forgotten_at IS NOT NULL) OR EXISTS (SELECT 1 FROM llm_wiki_events le WHERE le.related_entry_id = e.id AND le.forgotten_at IS NULL));`
Expected: the live subquery uses `llm_wiki_events_related_live_idx` (pre-fix: `SCAN le`). Paste the BEFORE and AFTER outputs verbatim into the spec's `## Errata` section with date + commit. Rationale: planner output is sqlite-version-dependent; the durable receipt is the recorded measurement, the unit test pins existence + guard.

- [ ] **Step 5: Run to green + commit** (one filter per invocation)

Run: `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib related_live_idx`
Then: `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib related_forgotten_idx`
Then: `npm test -- engine9-setup-compat   # frontend parity gate (vitest run)`

```bash
git add src-tauri/src/db/okf_ddl.rs src-tauri/src/db/connection.rs src-tauri/src/db/entities.rs src-tauri/src/db/events.rs src-tauri/src/wisdom_match.rs src/__tests__/engine9-setup-compat.test.ts
git commit -m "perf(gate): live-half partial index llm_wiki_events_related_live_idx + parity allowlist (#401)"
```

### Task 7: full gates + spec status

- [ ] **Step 1:** `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib --features test-utils` — green.
- [ ] **Step 2:** `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --test okf_bundle --features test-utils` — green (r2-M1: the INTEGRATION target — `--lib` gates cannot see it).
- [ ] **Step 3:** `CARGO_TARGET_DIR=… cargo test --manifest-path tools/Cargo.toml --lib` — green (r3 M-A1: the tools crate has NO `[features]` table — `--features test-utils` fails with "package does not contain this feature". Plain `--lib`; its consumers of `entry_not_forgotten_arm` see an unchanged signature).
- [ ] **Step 4:** `CARGO_TARGET_DIR=… cargo clippy --manifest-path src-tauri/Cargo.toml --lib --features test-utils -- -D warnings` — no NEW findings vs base.
- [ ] **Step 5:** `npm test` (full vitest) — green.
- [ ] **Step 6:** Flip spec status to `Implemented`; final LEDGER entry; push; CI green on the head commit; then `gh pr ready 402` ONLY after implementation + local gates complete (board rule: ready flip is `gh pr ready <N>` ONLY — never REST PATCH).
- [ ] **Step 7:** Commit: `git commit -m "docs(spec): flip forgotten-gate-followups spec to Implemented (#398)"` (cleanup commit deletes plan.md + LEDGER.md in the SAME commit).

## Self-Review

- Spec coverage: Task 1-4 = spec Tasks 1-2 (round-trip, incl. r1-M1 export probe + guarded dup reconciliation); Task 5 = spec Task 3 (+ closes #399 with the DROP TABLE meta characterization; both public entries per r2-B1); Task 6 = spec Task 4 (+ r1-B3 helper, r2 minor 2 correlated EQP). Test plan items 1-7 map to Tasks 4,1,1,5,6,6,7. ✓
- Type consistency: `append_event_id_comment_forgotten(Option<i64>)` Tasks 1→2; `parse_event_id_comment` 3-tuple Task 1→3; `entry_not_forgotten_arm_for_temporal(&Temporal, &str)` Task 5 internal; `TraversalGate` resolved in both public entries, held by `CompositeWalk`; `drop_forgotten_column` created in Task 2 (r3 M-A3), used by Tasks 2/4/6. ✓
- Placeholders: the two test blocks with prose bodies (bundle_read fixture, connection.rs re-apply) explicitly instruct verbatim-copy of existing in-file fixtures — permitted pattern (copy, don't invent). ✓
- r1 + r2 + r3 + r4 findings: all BLOCKERs, MAJORs, and minors folded with `r1-*`…`r4-*` tags inline. ✓
