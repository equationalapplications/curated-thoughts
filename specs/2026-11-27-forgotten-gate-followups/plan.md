# Forgotten-gate follow-ups Implementation Plan (#398, folds #399/#400/#401)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Carry `forgotten_at` through the OKF bundle round-trip, compute gate arms once per traversal, and add the live-half partial index.

**Architecture:** Three verticals in one PR: (1) bundle round-trip — the flag rides the log-line HTML comment grammar, applied verbatim on import behind a one-shot column probe; (2) traversal — a `TraversalGate` struct resolved once at the public walk entry and passed down through every gate site; (3) schema — a sibling partial index mirroring the forgotten-half index's guard.

**Tech Stack:** Rust (rusqlite, src-tauri crate), TypeScript vitest (engine-parity allowlist).

**Spec:** `specs/2026-11-27-forgotten-gate-followups/spec.md` (owner-approved 2026-11-27; the plan argues from it).

**Evidence base:** all file:line against `origin/main @ fc715239`.

## Global Constraints

- One canonical build location: `export CARGO_TARGET_DIR=/home/kv-thinkpad-t420-ubuntu/code/github/equationalapplications/curated-thoughts/.cargo-shared-target` (ABSOLUTE — repo AGENTS.md). Pre-flight `df -h` ≥30G free.
- Gate semantics frozen: fail-closed scoring-path contract, survival-guard predicate, carve-outs unchanged (spec Non-goals).
- `temporal_columns_shape_only` is the ONLY probe the gate uses; gate reads never touch `llm_wiki_meta`.
- Owner note 1 (Kurt 2026-11-27): parse `forgotten:` strictly inside the trailing `<!-- … -->` comment; summary text containing `forgotten:` is never parsed; existing id-only lines byte-identical.
- Owner note 2 (Kurt 2026-11-27): EVERY public traversal entry point passes walk-start arms down — no secondary path left probing per node.
- No new issues from this lane — findings fold into PR #402.
- Spec's `Errata` section is the durable home for EQP measurements (plan.md/LEDGER.md are deleted at cleanup).

## File Structure

- `src-tauri/src/okf/log_md.rs` — comment grammar (write + parse). Task 1.
- `src-tauri/src/okf/event_line.rs` — `build_event_text` signature + tests. Task 2.
- `src-tauri/src/okf/bundle_write.rs` — `ExportEvent` field, log-entry build. Task 2.
- `src-tauri/src/db/bundle_io.rs` — export SELECT. Task 2.
- `src-tauri/src/okf/bundle_read.rs` — `ParsedEvent` field + populate. Task 3.
- `src-tauri/src/db/bundle_apply.rs` — insert + shape probe + round-trip test. Task 4.
- `src-tauri/src/wisdom_match.rs` — `entry_not_forgotten_arm_for_temporal` helper. Task 5.
- `src-tauri/src/wiki_graph.rs` — `TraversalGate` + pass-down + tests. Task 5.
- `src-tauri/src/db/okf_ddl.rs` — live-half index. Task 6.
- `src-tauri/src/db/connection.rs` — guard test + expected-list row. Task 6.
- `src/__tests__/engine9-setup-compat.test.ts` — CT_OWNED allowlist row. Task 6.

---

### Task 1: log-line comment grammar — `forgotten:` key (log_md.rs)

**Files:**
- Modify: `src-tauri/src/okf/log_md.rs:33-70`
- Test: `src-tauri/src/okf/log_md.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Produces: `pub fn append_event_id_comment_forgotten(text: &str, event_id: &str, forgotten_at: Option<i64>) -> String`; `parse_event_id_comment` return type becomes `(String, Option<String>, Option<i64>)` — `(stripped_text, event_id, forgotten_at)`. All callers updated in Tasks 2–3.

Grammar: `({event_type}) {summary} <!-- id: evt_x forgotten:1732713600000 -->` — the key is `forgotten:` with NO space, digits only, inside the trailing comment (the parser already keys on the LAST `<!--` in the line, which satisfies the owner's strict-scoping note: `forgotten:` occurrences in summary text are never seen because only the trailing comment is inspected).

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
fn parse_reads_forgotten_from_trailing_comment_only() {
    // `forgotten:` in the summary must NOT be parsed (owner note 1).
    let (text, id, forgotten) = parse_event_id_comment(
        "(note) forgot: nope <!-- id: evt_x forgotten:1732713600000 -->",
    );
    assert_eq!(text, "(note) forgot: nope");
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
fn parse_non_numeric_forgotten_parses_as_absent() {
    let (text, id, forgotten) = parse_event_id_comment(
        "(note) hi <!-- id: evt_x forgotten:notanumber -->",
    );
    assert_eq!(id.as_deref(), Some("evt_x"));
    assert_eq!(forgotten, None);
    assert_eq!(text, "(note) hi");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --manifest-path src-tauri/src-tauri/../Cargo.toml okf::log_md 2>/dev/null || cargo test -p curated-thoughts okf::log_md` — use the canonical invocation:
`CARGO_TARGET_DIR=<repo>/.cargo-shared-target cargo test --manifest-path src-tauri/Cargo.toml --lib okf::log_md`
Expected: FAIL — `append_event_id_comment_forgotten` undefined; parse arity mismatch.

- [ ] **Step 3: Implement**

`append_event_id_comment_forgotten` — copy the id-validation guard from `append_event_id_comment` (`log_md.rs:33-42`), then:

```rust
pub fn append_event_id_comment_forgotten(
    text: &str,
    event_id: &str,
    forgotten_at: Option<i64>,
) -> String {
    match forgotten_at {
        Some(ms) if ms >= 0 => format!("{text} <!-- id: {event_id} forgotten:{ms} -->"),
        _ => append_event_id_comment(text, event_id),
    }
}
```

`parse_event_id_comment` — keep the existing last-`<!--` discovery and id validation EXACTLY (byte-identical stripped text); after extracting `inner`, replace the single-token read with a token loop:

```rust
let mut event_id: Option<String> = None;
let mut forgotten_at: Option<i64> = None;
for tok in inner.split_whitespace() {
    if let Some(v) = tok.strip_prefix("id:") {
        event_id = Some(v.to_string());
    } else if let Some(v) = tok.strip_prefix("forgotten:") {
        forgotten_at = v.parse::<i64>().ok();
    }
}
```

Return `(stripped, event_id, forgotten_at)`. The id validity check (alphanumeric/`.`/`_`/`-`) gates the whole comment exactly as today: an invalid id → `(text, None, None)` — wait, preserve today's exact behavior: today an invalid id returns `(stripped, None)`; keep that and return `forgotten_at: None` alongside. A negative or non-numeric `forgotten:` parses as absent (`parse::<i64>().ok()` handles both).

- [ ] **Step 4: Fix the in-file caller(s) and run to green**

`parse_event_id_comment`'s other caller is `event_line.rs:29` (`parse_event_text`) — update its destructure to the 3-tuple there (Task 3 finishes the plumbing; here just destructure and ignore: `let (without_id, event_id, _forgotten) = …`). Run the Task 1 tests: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/okf/log_md.rs src-tauri/src/okf/event_line.rs
git commit -m "feat(okf): forgotten: key in the event-line id comment grammar (#398)"
```

### Task 2: export side — ExportEvent carries the flag (bundle_io.rs, bundle_write.rs, event_line.rs)

**Files:**
- Modify: `src-tauri/src/okf/bundle_write.rs:18-29` (struct), `:159-176` (log build)
- Modify: `src-tauri/src/db/bundle_io.rs:222-241` (load_events)
- Modify: `src-tauri/src/okf/event_line.rs:11-27` (build_event_text), `:60-110` (tests)

**Interfaces:**
- Consumes: `append_event_id_comment_forgotten` (Task 1).
- Produces: `ExportEvent { …, pub forgotten_at: Option<i64> }`; `build_event_text(event_type: &str, summary: &str, related_path: Option<&str>, event_id: &str, forgotten_at: Option<i64>) -> String`.

- [ ] **Step 1: Write the failing test** (event_line.rs tests)

```rust
#[test]
fn build_event_text_carries_forgotten_key() {
    let text = build_event_text("action", "did a thing", None, "evt_x", Some(1000));
    assert_eq!(text, "(action) did a thing <!-- id: evt_x forgotten:1000 -->");
    let text = build_event_text("action", "did a thing", None, "evt_x", None);
    assert_eq!(text, "(action) did a thing <!-- id: evt_x -->");
}
```

- [ ] **Step 2: Run to verify failure** — same invocation as Task 1, filter `okf::event_line`. Expected: FAIL (arity).

- [ ] **Step 3: Implement**

`event_line.rs`: add the `forgotten_at: Option<i64>` param; final line becomes
`append_event_id_comment_forgotten(&core, event_id, forgotten_at)`.
`bundle_write.rs:18-29`: add `pub forgotten_at: Option<i64>,` to `ExportEvent` (doc comment: `/// Engine `forget()` tombstone, ms epoch; `None` = live. Rides the id comment on export (#398).`).
`bundle_write.rs:172` (`build_event_text(...)` call in the log_entries map): pass `ev.forgotten_at`.
`bundle_io.rs:226-234`: SELECT adds `forgotten_at`; row closure reads `let forgotten_at: Option<i64> = r.get(5)?;` and populates the field.

- [ ] **Step 4: Run event_line + bundle_write + bundle_io tests** — PASS (bundle_io has a `#[cfg(test)]` module with `use super::*;` — its `ExportEvent` constructions, if any, gain the field; run `cargo test --lib okf:: db::bundle_io` to catch them).

Run: `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib okf:: bundle_io`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/okf/event_line.rs src-tauri/src/okf/bundle_write.rs src-tauri/src/db/bundle_io.rs
git commit -m "feat(bundle): export carries forgotten_at in ExportEvent + log-line grammar (#398)"
```

### Task 3: parse side — ParsedEvent carries the flag (bundle_read.rs)

**Files:**
- Modify: `src-tauri/src/okf/bundle_read.rs:18-25` (struct), `:245-273` (populate)

**Interfaces:**
- Consumes: Task 1's 3-tuple `parse_event_id_comment`.
- Produces: `ParsedEvent { …, pub forgotten_at: Option<i64> }`.

- [ ] **Step 1: Write the failing test** (bundle_read.rs test module — it has one; follow its fixture conventions)

```rust
#[test]
fn parse_populates_forgotten_from_comment() {
    // Build a minimal bundle dir whose entity log carries one forgotten and
    // one live event line; assert ParsedEvent.forgotten_at values.
    // Reuse the module's existing fixture helpers; the log line for the
    // forgotten event is exactly:
    //   "(action) did a thing <!-- id: evt_a forgotten:1732713600000 -->"
    // assert: events[0].forgotten_at == Some(1732713600000)
    //         events[1].forgotten_at == None
}
```

(Implementer: copy the module's existing bundle-dir fixture setup verbatim; only the two log lines and the assertions are new.)

- [ ] **Step 2: Run to verify failure** — filter `okf::bundle_read`. Expected: FAIL (field missing).

- [ ] **Step 3: Implement**

`bundle_read.rs:18-25`: add `pub forgotten_at: Option<i64>,` to `ParsedEvent`.
`bundle_read.rs:245-273`: `parse_event_text`'s destructure gains the third slot; `entity.events.push(ParsedEvent { …, forgotten_at })`.

- [ ] **Step 4: Run to green** — `cargo test --lib okf::` PASS. Any other `ParsedEvent` constructions live in bundle_apply.rs tests (`:1137`) — they gain `forgotten_at: None` in Task 4.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/okf/bundle_read.rs
git commit -m "feat(bundle): ParsedEvent carries forgotten_at from the id comment (#398)"
```

### Task 4: import side — apply verbatim behind a one-shot shape probe (bundle_apply.rs)

**Files:**
- Modify: `src-tauri/src/db/bundle_apply.rs:371-385` (probe), `:712-753` (insert), `:1137` (test fixture field)

**Interfaces:**
- Consumes: `ParsedEvent.forgotten_at` (Task 3); `crate::db::ddl_compat::existing_columns`.

- [ ] **Step 1: Write the failing round-trip test** (#398 acceptance, bundle_apply.rs tests)

```rust
#[test]
fn round_trip_preserves_forgotten_at_and_gate_hides_entry() {
    // 1. Source brain (engine9_migrated fixture pattern — copy from the
    //    connection.rs guard test): seed one curated entity, one live entry
    //    `fact_a` with two llm_wiki_events rows `evt_live`, `evt_dead`
    //    (related_entry_id = 'fact_a', created_at = now). Engine-shape
    //    forget: UPDATE llm_wiki_events SET forgotten_at = <now_ms>
    //    WHERE id = 'evt_dead'.
    // 2. Export via bundle_io::export (the module's export entry; copy the
    //    existing export test's invocation) to a temp dir.
    // 3. Apply to a FRESH temp brain via apply_import(&mut conn, &bundle,
    //    ImportMode::Merge).
    // 4. Assert: imported evt_dead row carries forgotten_at (query the row);
    //    the wisdom gate hides fact_a on the importing brain:
    //    SELECT COUNT(*) FROM llm_wiki_entries e WHERE e.id='fact_a'
    //      AND e.deleted_at IS NULL ||forgotten_entry_arm(&shape)|| == 0.
    // 5. Assert evt_live imports live (forgotten_at IS NULL).
}
```

Plus the back-compat test:

```rust
#[test]
fn pre_v19_target_imports_without_forgotten_column() {
    // Fresh fixture, ALTER TABLE llm_wiki_events DROP COLUMN forgotten_at
    // (mirror connection.rs:8212), apply a bundle whose event carries
    // Some(ms) — assert: no error, events_added == 1, row's created_at
    // correct, and no forgotten_at reference broke.
}

#[test]
fn legacy_bundle_without_key_imports_live() {
    // apply_import a ParsedBundle whose ParsedEvent.forgotten_at == None
    // onto a v19 brain — assert row's forgotten_at IS NULL.
}
```

- [ ] **Step 2: Run to verify failure** — filter `bundle_apply::tests::round_trip` etc. Expected: FAIL (round-trip loses the flag).

- [ ] **Step 3: Implement**

In `apply_import` (`:371`), after `ImmediateTx::begin` (:384), probe ONCE:

```rust
// #398: one column probe per import, not per event. Pre-v19 targets
// (no forgotten_at) import without the column — unchanged behavior.
let events_have_forgotten_at = crate::db::ddl_compat::existing_columns(
    tx.as_ref(), "llm_wiki_events",
)?.iter().any(|c| c == "forgotten_at");
```

(`tx.as_ref()` — ImmediateTx derefs to Connection; use whichever form type-checks, e.g. `&tx`.)

In the event loop (:712-753), replace the single INSERT with:

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

Dup/merge logic above (:714-733) is UNCHANGED — a same-id event on the target is still `events_skipped`. Update the `:1137` test fixture's `ParsedEvent` construction with `forgotten_at: None`.

- [ ] **Step 4: Run to green** — `cargo test --lib bundle_apply` PASS (all three new tests + existing).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/db/bundle_apply.rs
git commit -m "feat(bundle): import applies forgotten_at verbatim behind one-shot shape probe (#398)"
```

### Task 5: gate arms once per traversal (wisdom_match.rs, wiki_graph.rs)

**Files:**
- Modify: `src-tauri/src/wisdom_match.rs:675-700` (`entry_not_forgotten_arm_for` split)
- Modify: `src-tauri/src/wiki_graph.rs:466-490` (`load_live_entry`), `:527-541` (`load_live_node`), `:541-707` (`fetch_neighbors` + outbound/inbound), `:779-1010` (entries), `:1150-1162` (`live_entry_exists`), tests `:1224-1227`, `:2306`

**Interfaces:**
- Produces (wisdom_match.rs): `pub(crate) fn entry_not_forgotten_arm_for_temporal(t: &Temporal, alias: &str) -> Result<String>` — the alias-validation + rewrite half of `entry_not_forgotten_arm_for`, factored verbatim.
- Produces (wiki_graph.rs): `struct TraversalGate { entry_arm: String, arm_s: String, arm_t: String }` + `fn resolve(conn: &Connection) -> Result<TraversalGate>`.

- [ ] **Step 1: Write the failing tests** (wiki_graph.rs tests)

```rust
#[test]
fn gate_probe_never_touches_llm_wiki_meta() {
    // Closes #399 item 1's verification claim. Build a v19-shape fixture,
    // install a tracing/probe: simplest reliable oracle — corrupt the meta
    // value the scheme resolver would read:
    //   INSERT OR REPLACE INTO llm_wiki_meta (key, value)
    //   VALUES ('wisdom_active_scheme', 'not-a-real-scheme');
    // A scheme-using read (temporal_columns) errors fail-closed on this.
    // The gate paths must still work:
    //   wiki_traverse_graph(...entry-space seed...) → Ok, forgotten entries
    //   excluded; live_entry_exists → Ok.
    // (temporal_columns_shape_only never reads meta — this test pins it.)
}

#[test]
fn traversal_arms_resolved_once_per_walk() {
    // Instrumentation oracle: use EXPLAIN-free counting — create the fixture,
    // run ONE wiki_traverse_graph walk over a 3-node chain, and assert via a
    // sqlite authorizer/trace that 'PRAGMA table_info' runs at most twice
    // (the single walk-start resolve) rather than per node. Use
    // rusqlite::trace via `conn.trace(Some(|s| …))` counting "PRAGMA
    // table_info(llm_wiki" occurrences; assert count <= 2 regardless of
    // node count (pre-fix this is ~4 per node).
}
```

- [ ] **Step 2: Run to verify failure** — filter `wiki_graph::unit_tests::gate_probe` + `traversal_arms`. Expected: FAIL (per-node probes; meta-free claim unproven by a test).

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
/// Walk-start gate arms (issue #400): resolved ONCE per public traversal
/// entry and passed down. A mid-walk schema change observes the walk-start
/// shape — accepted (spec §Task 3). Owner note 2: both public entries
/// (`wiki_traverse_graph` is the ONLY one; scoped + cross-partition are
/// private dispatches under it) share this resolve.
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

- `wiki_traverse_graph` (:779): `let gate = TraversalGate::resolve(conn)?;` first line; pass `&gate` to both private walkers.
- `scoped_traverse` / `cross_partition_traverse`: take `gate: &TraversalGate`; pass into `load_live_node`, `load_live_entry` (param `arm: &str` → uses `gate.entry_arm`), `fetch_neighbors`, `live_entry_exists`.
- `fetch_neighbors` → `fetch_outbound_neighbors` / `fetch_inbound_neighbors`: take `gate: &TraversalGate`, replace the per-call `entry_not_forgotten_arm_for` with `gate.arm_s` / `gate.arm_t`.
- `live_entry_exists` (:1150): take `arm: &str`, drop its probe.
- The `NodeSpace::Entity` path (`fetch_entity_neighbors`, `:711`) does NOT gate — it joins `curated_entities`, which has no `forgotten_at`; leave unchanged (note this in the plan-following code comment at `TraversalGate`).
- Update in-file tests `:1224-1227`, `:2306` call sites with the new signatures.

- [ ] **Step 4: Run to green** — `cargo test --lib wiki_graph` PASS, including the existing characterization tests `forgotten_gate_closes_graph_bypass_sites`, `forgotten_neighbor_edges_are_gated_in_traversal`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/wisdom_match.rs src-tauri/src/wiki_graph.rs
git commit -m "perf(traversal): gate arms resolved once per walk, passed down (#400, closes #399)"
```

### Task 6: live-half partial index (#401)

**Files:**
- Modify: `src-tauri/src/db/okf_ddl.rs:379-401`
- Modify: `src-tauri/src/db/connection.rs:8186-8215` (expected list + guard test)
- Modify: `src/__tests__/engine9-setup-compat.test.ts:247-256` (CT_OWNED set)

**Interfaces:** none consumed; produces index `llm_wiki_events_related_live_idx`.

- [ ] **Step 1: Write the failing tests**

connection.rs (mirror `related_forgotten_idx_is_guarded_on_both_columns`, `:8200`, in the SAME style):

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
    conn.execute_batch(
        "DROP INDEX llm_wiki_events_related_live_idx;
         ALTER TABLE llm_wiki_events DROP COLUMN forgotten_at;",
    ).unwrap();
    let conn2 = /* re-run apply_llm_wiki_v18_v19_partial_indexes on a fresh
                   pre-v19-shape fixture — mirror the forgotten test's second
                   half (:8216+), asserting idx_count == 0 and no error */;
}
```

(Implementer: copy the forgotten-index test's post-DROP re-apply block verbatim; substitute the index name.)

engine9-setup-compat.test.ts — add to `CT_OWNED`:

```typescript
"index:llm_wiki_events_related_live_idx",
```

Add `("index", "llm_wiki_events_related_live_idx")` to the Rust expected list at `connection.rs:8194` (the vec ending `llm_wiki_events_related_forgotten_idx`).

- [ ] **Step 2: Run to verify failure** — filter `related_live_idx`. Expected: FAIL (index not created).

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

Run against an engine9 fixture brain:
`EXPLAIN QUERY PLAN SELECT 1 FROM llm_wiki_events le WHERE le.related_entry_id='fact_x' AND le.forgotten_at IS NULL;`
Expected: `USING INDEX llm_wiki_events_related_live_idx` (pre-fix: `SCAN le`). Paste the BEFORE and AFTER outputs verbatim into the spec's new `## Errata` section with date + commit. Rationale: planner output is sqlite-version-dependent; the durable receipt is the recorded measurement, the unit test pins existence + guard.

- [ ] **Step 5: Run to green + commit**

```bash
CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib related_live_idx related_forgotten_idx
npm test -- engine9-setup-compat   # frontend parity gate (vitest run)
git add src-tauri/src/db/okf_ddl.rs src-tauri/src/db/connection.rs src/__tests__/engine9-setup-compat.test.ts
git commit -m "perf(gate): live-half partial index llm_wiki_events_related_live_idx + parity allowlist (#401)"
```

### Task 7: full gates + spec status

- [ ] **Step 1:** `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib --features test-utils` — green.
- [ ] **Step 2:** `CARGO_TARGET_DIR=… cargo test --manifest-path tools/Cargo.toml --lib --features test-utils` — green (tools crate consumes `entry_not_forgotten_arm`; unchanged signature, but verify).
- [ ] **Step 3:** `CARGO_TARGET_DIR=… cargo clippy --manifest-path src-tauri/Cargo.toml --lib --features test-utils -- -D warnings` — no NEW findings vs base.
- [ ] **Step 4:** `npm test` (full vitest) — green.
- [ ] **Step 5:** Flip spec status to `Implemented`; final LEDGER entry; push; CI green on the head commit; then `gh pr ready 402` ONLY after implementation + local gates complete (board rule: ready flip is `gh pr ready <N>` ONLY — never REST PATCH).
- [ ] **Step 6:** Commit: `git commit -m "docs(spec): flip forgotten-gate-followups spec to Implemented (#398)"` (cleanup commit deletes plan.md + LEDGER.md in the SAME commit).

## Self-Review

- Spec coverage: Task 1-4 = spec Tasks 1-2 (round-trip); Task 5 = spec Task 3 (+ closes #399 with the no-meta-read test); Task 6 = spec Task 4. Test plan items 1-7 map to Tasks 4,1,1,5,6,6,7. ✓
- Type consistency: `append_event_id_comment_forgotten(Option<i64>)` used in Tasks 1→2; `parse_event_id_comment` 3-tuple Task 1→3; `entry_not_forgotten_arm_for_temporal(&Temporal, &str)` Task 5 internal; `TraversalGate` Task 5 internal. ✓
- Placeholders: the two test blocks with prose bodies (bundle_read fixture, connection.rs re-apply) explicitly instruct verbatim-copy of existing in-file fixtures — permitted pattern (copy, don't invent). ✓
