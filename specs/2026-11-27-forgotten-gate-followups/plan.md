# Forgotten-gate follow-ups Implementation Plan (#398, folds #399/#400/#401)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Carry `forgotten_at` through the OKF bundle round-trip, compute gate arms once per traversal, and add the live-half partial index.

**Architecture:** Three verticals in one PR: (1) bundle round-trip — the flag rides the log-line HTML comment grammar, applied verbatim on import behind a one-shot column probe, with dup-row reconciliation so an already-synced brain cannot resurrect a forgotten event; (2) traversal — a `TraversalGate` struct resolved once at the public walk entry and passed down through every gate site; (3) schema — a sibling partial index mirroring the forgotten-half index's guard.

**Tech Stack:** Rust (rusqlite, src-tauri crate), TypeScript vitest (engine-parity allowlist).

**Spec:** `specs/2026-11-27-forgotten-gate-followups/spec.md` (owner-approved 2026-11-27; amended r1 2026-11-27 — round-trip fixture, export probe, dup reconciliation, one-way compat note, entry wording; the plan argues from the amended text).

**Evidence base:** all file:line against `origin/main @ fc715239`.

**Revision history:**
- r1 (2026-11-27, Opus-medium plan review, $0.90/24 turns, REQUEST CHANGES — all findings verified against the tree and folded): **B1** parser token loop dropped ids → id-first extraction restored; **B2** round-trip fixture defeated by the survival guard's live-half OR → two-entry fixture; **B3** new partial index breaks five DROP COLUMN test sites → drop-list + shared helper; **M1** export SELECT unconditional → one-shot probe + `NULL AS forgotten_at`; **M2** old-binary id loss on new-format lines → one-way compat recorded in spec; **M3** dup-skip resurrection on synced brains → UPDATE reconciliation; **M4** trace-oracle uncompilable → DROP TABLE meta oracle + structural pass-down; minors 1–8 folded (test strings, negative filter, writer guard on both paths, cargo commands, call sites, oracle strength, entry wording, format! in test sketch).

## Global Constraints

- One canonical build location: `export CARGO_TARGET_DIR=/home/kv-thinkpad-t420-ubuntu/code/github/equationalapplications/curated-thoughts/.cargo-shared-target` (ABSOLUTE — repo AGENTS.md). Pre-flight `df -h` ≥30G free.
- Gate semantics frozen: fail-closed scoring-path contract, survival-guard predicate, carve-outs unchanged (spec Non-goals).
- `temporal_columns_shape_only` is the ONLY probe the gate uses; gate reads never touch `llm_wiki_meta`.
- Owner note 1 (Kurt 2026-11-27): parse `forgotten:` strictly inside the trailing `<!-- … -->` comment; summary text containing `forgotten:` is never parsed; existing id-only lines byte-identical.
- Owner note 2 (Kurt 2026-11-27): EVERY public traversal entry point passes walk-start arms down — no secondary path left probing per node.
- **DROP COLUMN + partial index (r1-B3):** SQLite refuses `ALTER TABLE … DROP COLUMN` while a partial index's WHERE clause uses the column. The live index adds a THIRD index to drop at every pre-v19-shape test fixture: `connection.rs:8212-8214`, `entities.rs:1306-1308`, `events.rs:231-233`, `wisdom_match.rs:1948-1950`, `wisdom_match.rs:2133-2135` (and the sibling near `:1965` if it drops the column). Add ONE shared test helper `drop_forgotten_column(conn)` (drops all three indexes, then the column) and use it at every site instead of repeating the list.
- No new issues from this lane — findings fold into PR #402.
- Spec's `Errata` section is the durable home for EQP measurements (plan.md/LEDGER.md are deleted at cleanup).

## File Structure

- `src-tauri/src/okf/log_md.rs` — comment grammar (write + parse). Task 1.
- `src-tauri/src/okf/event_line.rs` — `build_event_text` signature + tests. Task 2.
- `src-tauri/src/okf/bundle_write.rs` — `ExportEvent` field, log-entry build. Task 2.
- `src-tauri/src/db/bundle_io.rs` — export SELECT + shape probe. Task 2.
- `src-tauri/src/okf/bundle_read.rs` — `ParsedEvent` field + populate. Task 3.
- `src-tauri/src/db/bundle_apply.rs` — insert + shape probe + dup reconciliation + round-trip test. Task 4.
- `src-tauri/src/wisdom_match.rs` — `entry_not_forgotten_arm_for_temporal` helper. Task 5.
- `src-tauri/src/wiki_graph.rs` — `TraversalGate` + pass-down + tests. Task 5.
- `src-tauri/src/db/okf_ddl.rs` — live-half index. Task 6.
- `src-tauri/src/db/connection.rs` — guard test + expected-list row + `drop_forgotten_column` helper. Task 6.
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

Return `(stripped, event_id, forgotten_at)`. Behavior for every line the old parser accepted is unchanged: same stripped text, same id; `forgotten_at` is `None` unless a valid key is present.

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
- Modify: `src-tauri/src/db/bundle_io.rs` (load_events `:222-241` + probe at the export entry)
- Modify: `src-tauri/src/okf/event_line.rs:11-27` (build_event_text), `:60-110` (tests)

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

bundle_io.rs (r1-M1 — pre-v19 export tolerance; follow the module's existing fixture pattern):

```rust
#[test]
fn export_tolerates_pre_v19_shape_without_forgotten_column() {
    // Engine9-migrated fixture, then drop the column via the
    // drop_forgotten_column helper (Task 6 — all three indexes first).
    // Export must succeed; events export with forgotten_at == None.
}
```

- [ ] **Step 2: Run to verify failure** (one filter per invocation — r1 minor 4)

Run: `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib okf::event_line`
Then: `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib bundle_io`
Expected: FAIL (arity; probe/test missing).

- [ ] **Step 3: Implement**

`event_line.rs`: add the `forgotten_at: Option<i64>` param; final line becomes
`append_event_id_comment_forgotten(&core, event_id, forgotten_at)`.
`bundle_write.rs:18-29`: add `pub forgotten_at: Option<i64>,` to `ExportEvent` (doc comment: `/// Engine `forget()` tombstone, ms epoch; `None` = live. Rides the id comment on export (#398).`).
`bundle_write.rs:172` (`build_event_text(...)` call in the log_entries map): pass `ev.forgotten_at`.
`bundle_io.rs` (r1-M1): the export entry probes ONCE with `crate::db::ddl_compat::existing_columns(conn, "llm_wiki_events")` and passes `has_forgotten: bool` into `load_events`; the SELECT appends `, forgotten_at` when present, `, NULL AS forgotten_at` when not (so `let forgotten_at: Option<i64> = r.get(5)?;` works for both shapes). A pre-v19 brain exports without error — matching the codebase's degraded-shape tolerance everywhere else (spec r1-M1 ruling).

- [ ] **Step 4: Run to green**

Run: `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib okf::`
Then: `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib bundle_io`
Expected: PASS (bundle_io's `#[cfg(test)]` `ExportEvent` constructions, if any, gain the field).

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

### Task 4: import side — apply verbatim behind a one-shot shape probe, with dup reconciliation (bundle_apply.rs)

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
    // 1. Source brain (engine9_migrated fixture pattern — copy from the
    //    connection.rs guard test): seed TWO live entries `fact_a` and
    //    `fact_b`, both source_type NOT 'user_stated' (carve-out!) and
    //    neither id == entity_id nor tier_* (carve-outs!). Events:
    //    evt_dead (related_entry_id='fact_a', forgotten_at set via the
    //    engine-shape UPDATE) and evt_live (related_entry_id='fact_b',
    //    forgotten_at NULL).
    // 2. Export via bundle_io (copy the existing export test's invocation)
    //    to a temp dir.
    // 3. Apply to a FRESH temp brain via apply_import(&mut conn, &bundle,
    //    ImportMode::Merge).
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

In the event loop (:712-753), keep the dup decision exactly as-is (:714-733), then reconcile a dup that arrived forgotten (r1-M3):

```rust
if is_dup {
    if let (Some(ms), Some(id)) = (ev.forgotten_at, ev.event_id.as_deref()) {
        // #398: a dup whose source row is forgotten must tombstone the
        // target's live row — otherwise an already-synced brain keeps the
        // forgotten event alive forever (the exact resurrection #398 fixes).
        tx.execute(
            "UPDATE llm_wiki_events SET forgotten_at = ?1
             WHERE id = ?2 AND forgotten_at IS NULL",
            params![ms, id],
        )?;
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

Semantics note (spec r1-M3 ruling): tombstone wins on conflict — a target row already forgotten is never un-forgotten by an incoming live event; only a live target row is tombstoned by an incoming forgotten event. Update the `:1137` test fixture's `ParsedEvent` construction with `forgotten_at: None`.

- [ ] **Step 4: Run to green** — `cargo test --manifest-path src-tauri/Cargo.toml --lib bundle_apply` PASS (all four new tests + existing).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/db/bundle_apply.rs
git commit -m "feat(bundle): import applies forgotten_at verbatim, reconciles dups (#398)"
```

### Task 5: gate arms once per traversal (wisdom_match.rs, wiki_graph.rs)

**Files:**
- Modify: `src-tauri/src/wisdom_match.rs:675-700` (`entry_not_forgotten_arm_for` split)
- Modify: `src-tauri/src/wiki_graph.rs:466-490` (`load_live_entry`), `:527-541` (`load_live_node`), `:541-707` (`fetch_neighbors` + outbound/inbound), `:779-1010` (entries), `:1150-1162` (`live_entry_exists`), test call sites `:1224-1227`, `:2274`, `:2306`, `:2344`

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
    //   conn.execute("DROP TABLE llm_wiki_meta") on a v19 fixture, then:
    //   wiki_traverse_graph (entry-space seed) → Ok, forgotten entries
    //   excluded; live_entry_exists → Ok. Scoring/retrieval paths that DO
    //   resolve the scheme keep failing closed (unchanged).
}

#[test]
fn traversal_gate_resolves_both_shapes() {
    // Unit test on TraversalGate::resolve: v19 fixture → all three arms
    // non-empty; pre-v19 fixture (drop_forgotten_column helper) → all
    // three arms EMPTY (gate inert, behavior identical to today).
    //
    // r1-M4: the once-per-walk property is enforced STRUCTURALLY, not by a
    // trace counter — fetch_*_neighbors / load_live_entry /
    // live_entry_exists take &TraversalGate or &str arm params, so no
    // per-node probe path exists to call (rusqlite's `trace` feature is
    // not enabled — workspace Cargo.toml:30 — and `Connection::trace`
    // takes a fn pointer, so a counting closure cannot compile; oracle
    // declined). The characterization tests below pin behavior.
}
```

Existing characterization tests must stay green:
`forgotten_gate_closes_graph_bypass_sites`, `forgotten_neighbor_edges_are_gated_in_traversal`.

- [ ] **Step 2: Run to verify failure** — `CARGO_TARGET_DIR=… cargo test --manifest-path src-tauri/Cargo.toml --lib wiki_graph`. Expected: FAIL (with the meta table dropped, any gate probe reading it errors — and TraversalGate doesn't exist).

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
/// Walk-start gate arms (issue #400): resolved ONCE at the public
/// traversal entry and passed down. A mid-walk schema change observes the
/// walk-start shape — accepted (spec §Task 3, amended r1). Owner note 2:
/// the ONLY public entry is `wiki_traverse_graph`; `scoped_traverse` and
/// `cross_partition_traverse` are private dispatches under it and both
/// receive the gate (spec's per-walker wording amended r1 to match —
/// deviation disclosed in the spec's revision history). The entity-space
/// path (`fetch_entity_neighbors`) does NOT gate: it joins
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

- `wiki_traverse_graph` (:779): `let gate = TraversalGate::resolve(conn)?;` first line; pass `&gate` to both private walkers.
- `scoped_traverse` / `cross_partition_traverse`: take `gate: &TraversalGate`; pass into `load_live_node`, `load_live_entry` (param `arm: &str` → uses `gate.entry_arm`), `fetch_neighbors`, `live_entry_exists`.
- `fetch_neighbors` → `fetch_outbound_neighbors` / `fetch_inbound_neighbors`: take `gate: &TraversalGate`, replace the per-call `entry_not_forgotten_arm_for` with `gate.arm_s` / `gate.arm_t`.
- `live_entry_exists` (:1150): take `arm: &str`, drop its probe.
- Update ALL in-file test call sites with the new signatures (r1 minor 5): `:1224-1227` (load_live_entry, live_entry_exists), `:2274` (load_live_node seed), `:2306` (fetch_neighbors), `:2344` (load_live_entry).

- [ ] **Step 4: Run to green** — `cargo test --manifest-path src-tauri/Cargo.toml --lib wiki_graph` PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/wisdom_match.rs src-tauri/src/wiki_graph.rs
git commit -m "perf(traversal): gate arms resolved once per walk, passed down (#400, closes #399)"
```

### Task 6: live-half partial index (#401)

**Files:**
- Modify: `src-tauri/src/db/okf_ddl.rs:379-401`
- Modify: `src-tauri/src/db/connection.rs:8186-8215` (expected list + guard test + `drop_forgotten_column` helper)
- Modify: the five pre-v19-shape test sites (see Global Constraints) to use the helper
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

Add `("index", "llm_wiki_events_related_live_idx")` to the Rust expected list at `connection.rs:8194`.

Introduce the shared helper (connection.rs `#[cfg(test)]`; if cross-file `pub(crate)` cfg(test) helpers don't resolve from the other test modules, duplicate the 5-line helper per test module — smallness over cleverness):

```rust
/// Drops the three forgotten_at partial indexes, then the column — the
/// exact order SQLite requires before ALTER TABLE … DROP COLUMN (r1-B3).
#[cfg(test)]
pub(crate) fn drop_forgotten_column(conn: &Connection) {
    conn.execute_batch(
        "DROP INDEX IF EXISTS llm_wiki_events_related_live_idx;
         DROP INDEX IF EXISTS llm_wiki_events_related_forgotten_idx;
         DROP INDEX IF EXISTS llm_wiki_events_forgotten_idx;
         ALTER TABLE llm_wiki_events DROP COLUMN forgotten_at;",
    ).unwrap();
}
```

Convert all five sites to use it: `connection.rs:8212-8214` (the forgotten guard test), `entities.rs:1306-1308`, `events.rs:231-233`, `wisdom_match.rs:1948-1950`, `wisdom_match.rs:2133-2135`.

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

Run against an engine9 fixture brain:
`EXPLAIN QUERY PLAN SELECT 1 FROM llm_wiki_events le WHERE le.related_entry_id='fact_x' AND le.forgotten_at IS NULL;`
Expected: `USING INDEX llm_wiki_events_related_live_idx` (pre-fix: `SCAN le`). Paste the BEFORE and AFTER outputs verbatim into the spec's new `## Errata` section with date + commit. Rationale: planner output is sqlite-version-dependent; the durable receipt is the recorded measurement, the unit test pins existence + guard.

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
- [ ] **Step 2:** `CARGO_TARGET_DIR=… cargo test --manifest-path tools/Cargo.toml --lib --features test-utils` — green (tools crate consumes `entry_not_forgotten_arm`; unchanged signature, but verify).
- [ ] **Step 3:** `CARGO_TARGET_DIR=… cargo clippy --manifest-path src-tauri/Cargo.toml --lib --features test-utils -- -D warnings` — no NEW findings vs base.
- [ ] **Step 4:** `npm test` (full vitest) — green.
- [ ] **Step 5:** Flip spec status to `Implemented`; final LEDGER entry; push; CI green on the head commit; then `gh pr ready 402` ONLY after implementation + local gates complete (board rule: ready flip is `gh pr ready <N>` ONLY — never REST PATCH).
- [ ] **Step 6:** Commit: `git commit -m "docs(spec): flip forgotten-gate-followups spec to Implemented (#398)"` (cleanup commit deletes plan.md + LEDGER.md in the SAME commit).

## Self-Review

- Spec coverage: Task 1-4 = spec Tasks 1-2 (round-trip, incl. r1-M1 export probe + r1-M3 dup reconciliation); Task 5 = spec Task 3 (+ closes #399 with the DROP TABLE meta test); Task 6 = spec Task 4 (+ r1-B3 helper). Test plan items 1-7 map to Tasks 4,1,1,5,6,6,7. ✓
- Type consistency: `append_event_id_comment_forgotten(Option<i64>)` Tasks 1→2; `parse_event_id_comment` 3-tuple Task 1→3; `entry_not_forgotten_arm_for_temporal(&Temporal, &str)` Task 5 internal; `TraversalGate` Task 5 internal; `drop_forgotten_column` Task 6 shared by Tasks 2/4/6 tests. ✓
- Placeholders: the two test blocks with prose bodies (bundle_read fixture, connection.rs re-apply) explicitly instruct verbatim-copy of existing in-file fixtures — permitted pattern (copy, don't invent). ✓
- r1 findings: all 3 BLOCKERs + 4 MAJORs + 8 minors folded with `r1-*` tags inline. ✓
