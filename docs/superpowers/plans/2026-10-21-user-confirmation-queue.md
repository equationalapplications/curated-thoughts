# User Confirmation Queue Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Chat-stated and app-stated facts become human-verifiable through an in-app confirmation queue; confirmed facts join the activity delivery aperture; all agent-facing recall results carry provenance labels.

**Architecture:** Two entry paths (Path A: agent deposits with attribution frontmatter; Path B: app-written `stated/` files with an attestation table) feed one queue query (live ∧ not-human-tier ∧ not-denied). Confirm/deny are app-only writes shaped on `drafts.rs:62-74`, mirrored to app-written JSONL ledgers in `stated/` for rebuild. Recall gains per-hit `source_type`/`verified` labels and a confidence ordinal; the #282 §5 aperture widens to `["user_stated", "user_confirmed"]` via `provenance_filter: Option<&[&str]>`.

**Tech Stack:** Rust (src-tauri), SQLite (brain), Tauri 2 desktop app (React/TS frontend), `ct` CLI (tools/).

**Spec:** `docs/superpowers/specs/2026-10-20-user-confirmation-queue-design.md` (rev 4.1 — the binding authority; the plan argues from it, and on conflict the spec wins).

## Global Constraints

- Working tree: branch `spec/user-confirmation-queue`, repo `/home/kv-thinkpad-t420-ubuntu/src/curated-thoughts`. NEVER push; the controller pushes.
- Rust gates are SERIAL: `CARGO_BUILD_JOBS=2`, shared target dir `.cargo-shared-target` (absolute `CARGO_TARGET_DIR` per repo AGENTS.md), debuginfo stripped; exactly ONE build tree; `target/` is banned.
- Every cargo gate redirects FULL output to a log file (never pipe-judge an exit code); `cargo check` gates include `--tests`.
- `pnpm install --frozen-lockfile` before the first frontend gate (ddl_compat fixture).
- Message mode is byte-identical: no `SCHEMA_VERSION` bump for any change in Tasks 1–9 except where a migration adds tables/columns (migrations do not bump the cached-output schema version); the `ct wisdom match` cached-output replay test must stay green after every task.
- Provenance vocabulary verbatim: `librarian_inferred`, `user_stated`, `user_confirmed`, `immutable_document` (`wisdom_match.rs:47-52`). Human tier = `user_stated` + `user_confirmed` only.
- `user_confirmed` is stamped ONLY where `human_surface: true` (Task 3). No other surface may pass it.
- New DB objects use one migration; table names: `stated_attestations`, `queue_denials`. Ledger files: `stated/.confirmations.jsonl`, `stated/.attestations.jsonl`, `stated/.denials.jsonl` (ingest tier NONE).
- Commit style: conventional commits, one commit per task, exact messages pinned per task.
- Harness noise: `bash: .../snap/code/.../bin/env: No such file or directory` lines are noise, not failures.

---

### Task 1: Attribution carrier — deposit frontmatter + deposit-time index

**Files:**
- Modify: `src-tauri/src/wisdom_deposit.rs` (`render_deposit_file` ~`:89-104`; `audit_and_seed_kick` closure ~`:454-480`)
- Modify: `src-tauri/src/wisdom_deposit.rs` (deposit argument struct — the type the MCP deposit tool and internal callers pass)
- Test: `src-tauri/src/wisdom_deposit.rs` (test module)

**Interfaces:**
- Produces: `DepositAttribution { speaker: String, kind: AttributionKind, source: String }`, `enum AttributionKind { Verbatim, RestatedAtDirection }`, `TryFrom<&str> for AttributionKind`, fn `validate_handle(&str) -> Result<(), String>` (regex `^[a-z]+:[A-Za-z0-9_.-]+$`); `render_deposit_file` gains `attribution: Option<&DepositAttribution>`; brain table `deposit_attribution(document_path TEXT PRIMARY KEY, speaker TEXT, kind TEXT, source TEXT, deposited_at TEXT)`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn attribution_frontmatter_renders_and_parses_back() {
    let a = DepositAttribution { speaker: "kurt".into(), kind: AttributionKind::RestatedAtDirection, source: "discord:1562137256498569278".into() };
    let md = render_test_deposit(Some(&a));          // harness helper: renders a minimal deposit note
    assert!(md.starts_with("---\nattributed_speaker: kurt\nattribution_kind: restated_at_direction\nattribution_source: discord:1562137256498569278\n---\n"));
    assert_eq!(AttributionKind::try_from("verbatim").unwrap(), AttributionKind::Verbatim);
    assert!(AttributionKind::try_from("paraphrase").is_err());
    assert!(validate_handle("discord:123").is_ok());
    assert!(validate_handle("has space:123").is_err());
    assert!(validate_handle("discord:").is_err());
}

#[test]
fn unattributed_deposit_renders_byte_identical() {
    assert_eq!(render_test_deposit(None), BASELINE_NOTE); // BASELINE_NOTE: exact pre-change render, pinned verbatim in the test
}

#[test]
fn attribution_index_lands_in_audit_and_seed_kick_transaction() {
    // scratch brain: deposit with attribution via the internal deposit path
    // assert deposit_attribution has the row AND kick ledger row exists in the SAME sqlite transaction scope (query both after)
}
```

- [ ] **Step 2: Run to verify RED**

Run: `cargo test -p curated-thoughts --lib wisdom_deposit 2>&1 | tee /tmp/t1.log` (with the repo gate wrapper/env per AGENTS.md). Expected: compile FAIL — `DepositAttribution` not found.

- [ ] **Step 3: Implement**

Add the struct/enum/validation; extend `render_deposit_file` to emit the three frontmatter lines when `attribution` is `Some` (else byte-identical output); inside the `audit_and_seed_kick` `with_rw` closure, insert the `deposit_attribution` row (migration in Task 2's migration file — coordinate: this task adds the migration SQL file, Task 2 extends it; if Task 2 has not landed, add ONLY `deposit_attribution` here).

- [ ] **Step 4: Gates**

`cargo check --tests` warning-free on touched files; full `wisdom_deposit` test module green; counts = baseline + 3.

- [ ] **Step 5: Commit** — `feat(deposit): attribution frontmatter carrier with deposit-time index`

### Task 2: Migration — `stated_attestations`, `queue_denials` + ledger JSONL writers/rebuild

**Files:**
- Create: `src-tauri/src/db/migrations/NNNN_stated_verification.sql` (next number; check `migrations/` for the current max)
- Modify: `src-tauri/src/db/mod.rs` (migration registration), new module `src-tauri/src/db/stated_ledger.rs`
- Test: `src-tauri/src/db/stated_ledger.rs` (test module)

**Interfaces:**
- Produces: `stated_attestations(document_path TEXT PRIMARY KEY, content_hash TEXT NOT NULL, attested_by TEXT NOT NULL, attested_at TEXT NOT NULL)`; `queue_denials(document_path TEXT NOT NULL, chunk_hash TEXT NOT NULL, fact_text_hash TEXT NOT NULL, denied_by TEXT NOT NULL, denied_at TEXT NOT NULL, PRIMARY KEY(document_path, chunk_hash, fact_text_hash))`; `fn record_confirmation(conn, fact_id, content_hash, by, at) -> Result<()>` (appends JSONL + returns the row to upsert); `fn rebuild_rederive(conn, vault_root) -> Result<RebuildReport>` (repopulates all three tables from the JSONL ledgers, re-verifying hashes; attestations mismatched → dropped).

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn confirmation_jsonl_append_and_rebuild_roundtrip() {
    // scratch brain + scratch vault root; record_confirmation twice; delete brain rows;
    // rebuild_rederive re-inserts; second rebuild is idempotent (counts unchanged)
}
#[test]
fn rebuild_drops_attestation_with_hash_mismatch() {
    // write .attestations.jsonl line for a file, then EDIT the file; rebuild → row dropped
}
#[test]
fn confirmation_rebuild_matches_by_content_hash_fact_id_advisory() {
    // ledger line whose fact_id does not exist but hash matches a rebuilt row → row confirmed
}
```

- [ ] **Step 2: RED** — same gate recipe. Expected: module/file missing → compile FAIL.
- [ ] **Step 3: Implement** — migration SQL (two tables, `IF NOT EXISTS`), registration, `stated_ledger` module (single-line JSON appends via `OpenOptions::append`; SHA-256 via the same `hasher::hash_bytes` routine the `documents` table uses).
- [ ] **Step 4: Gates** — `cargo check --tests`; `stated_ledger` + migration tests green (baseline + 3).
- [ ] **Step 5: Commit** — `feat(db): stated verification tables and app-written jsonl ledgers`

### Task 3: Human-tier hardening — `human_surface` flag + provenance arms

**Files:**
- Modify: `src-tauri/src/db/commit.rs` (`ResolveOptions` struct ~`:39`; `resolve_proposal` arm `:2926-2930`)
- Modify: `src-tauri/src/db/proposals_review.rs` (`:161-168`, `:263-265` — the two `auto_approve:false` call sites pass `human_surface: false`; the DESKTOP caller passes `true`; test at `:633-640` updated)
- Modify: `src-tauri/src/proposals_api.rs` (`:21` — `DESKTOP_REVIEWER` stays a label; add the flag to `ResolveOptions` construction)
- Test: `src-tauri/src/db/proposals_review.rs` (test module)

**Interfaces:**
- Produces: `ResolveOptions { ..., human_surface: bool }` (default `false` via existing construction sites); `resolve_proposal` provenance decision: (1) if `human_surface && auto_approve == false` → `user_confirmed`; (2) else if `!human_surface && auto_approve == false` → `librarian_inferred` + event `approved_at_agent_tier`; (3) `auto_approve == true` → `librarian_inferred` (unchanged). Path B's `user_stated` arm lands in Task 8 (same decision point).

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn desktop_surface_stamps_user_confirmed() { /* auto_approve:false + human_surface:true → provenance user_confirmed, confidence unchanged at proposal stage */ }
#[test]
fn cli_and_internal_approvals_resolve_at_agent_tier() {
    // auto_approve:false, human_surface:false → librarian_inferred + event approved_at_agent_tier
    // includes the UPDATED legacy case: reviewed_by="tessera" now yields agent tier
}
#[test]
fn auto_approve_unchanged() { /* librarian_inferred, no event */ }
```

- [ ] **Step 2: RED** → **Step 3: Implement** (arm reorder per spec §3.7-A; thread ALL `ResolveOptions` construction sites in one pass; gate with `cargo check --tests`) → **Step 4: Gates** (baseline + 3) → **Step 5: Commit** — `feat(wisdom): human-tier stamps require the desktop surface`

### Task 4: Confirmation queue query + read-only MCP listing

**Files:**
- Modify: `src-tauri/src/db/queue.rs` (new file — queue query)
- Modify: `src-tauri/src/tool_dispatch.rs` (new read-only tool `curated_queue_list`, registered beside `curated_proposals_list` ~`:1310`; NO mutate tool)
- Test: `src-tauri/src/db/queue.rs` (test module)

**Interfaces:**
- Produces: `QueueRow { fact_id, title, body, provenance, entity_id, evidence_quote: Option<String>, source_document: Option<String>, attributed_speaker: Option<String>, instruction_source: Option<String>, age_days }`; fn `queue_rows(conn) -> Vec<QueueRow>` — SQL: live entries (`lifecycle_status='stable'`, not superseded/expired per the existing supersede filters) with `source_type NOT IN ('user_stated','user_confirmed')` (NULL/out-of-vocab included) LEFT JOIN anti-match against `queue_denials` (document path + chunk hash + fact-text hash), evidence quote via the `deposit_status_row` join chain (`wisdom_deposit.rs:183-187`), ORDER BY age DESC.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn queue_membership_live_not_human_tier() { /* librarian_inferred + immutable_document in; user_stated/user_confirmed out; NULL source_type in; superseded/expired out */ }
#[test]
fn denied_fact_excluded_sibling_included() { /* deny (doc,chunk,textA): fact A out, sibling fact B (same doc+chunk, different text) STILL in */ }
#[test]
fn evidence_quote_and_attribution_surfaces_on_row() { /* attributed deposit row shows speaker + quote */ }
```

- [ ] **Step 2: RED** → **Step 3: Implement** → **Step 4: Gates** (baseline + 3; also run the MCP tool registration test module) → **Step 5: Commit** — `feat(queue): confirmation queue query and read-only mcp listing`

### Task 5: App confirm/deny actions (stamp + suppression + ledger append)

**Files:**
- Modify: `src-tauri/src/db/queue.rs` (actions)
- Modify: `src-tauri/src/db/drafts.rs` (reuse the `:62-74` UPDATE shape — extract or mirror exactly)
- Test: `src-tauri/src/db/queue.rs` (test module)

**Interfaces:**
- Produces: `fn confirm_fact(app_ctx, fact_id) -> Result<()>` — one UPDATE: `source_type='user_confirmed', confidence='confirmed'`, `okf_verified` json_insert `{by:"human:kurt", at}`, `last_verified_by`/`last_verified_at` mirror, outbox Update row, event; `lifecycle_status` UNTOUCHED; `.confirmations.jsonl` append; idempotent on re-confirm. `fn deny_fact(app_ctx, fact_id) -> Result<()>` — archive + `queue_denials` insert + `.denials.jsonl` append. `fn undeny_fact(app_ctx, denial_key) -> Result<()>`. These are exposed to the FRONTEND ONLY (Tauri command layer, Task 9) — no MCP/CLI wrapper.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn confirm_stamps_the_full_shape() { /* provenance, confidence, verified list, last_verified_by/at, outbox row, event, jsonl line; lifecycle_status untouched */ }
#[test]
fn confirm_is_idempotent() { /* second confirm: no-op, no duplicate event, no duplicate jsonl line */ }
#[test]
fn deny_suppresses_and_undeny_restores() { /* deny → out of queue; undeny → back; jsonl line written */ }
```

- [ ] **Step 2: RED** → **Step 3: Implement** → **Step 4: Gates** (baseline + 3) → **Step 5: Commit** — `feat(queue): app-only confirm and deny with durable ledgers`

### Task 6: Active Verification Prompting — emitter + drop file

**Files:**
- Modify: `src-tauri/src/tool_dispatch.rs` (emit hook inside the three recall paths, after results are computed, BEFORE returning — never blocking)
- Create: `src-tauri/src/prompt_drop.rs`
- Test: `src-tauri/src/prompt_drop.rs` (test module)

**Interfaces:**
- Produces: `fn emit_unverified_match(vault_root, ev: UnverifiedMatch) -> ()` — infallible from the caller's perspective (I/O errors logged + swallowed); appends JSONL `{fact_id, title, body, quote, query, surface, ts}` to `<vault>/.brain/unverified-match.jsonl` ONLY if no UNACKED line for that fact_id (read-before-append); app-side drain/ack contract documented in the module doc comment.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn emitter_writes_jsonl_and_dedupes_unacked() { /* two emits, one line */ }
#[test]
fn read_only_brain_dir_never_errors_the_call() { /* chmod 0o500 .brain (restore in catch/finally); emit returns normally, no line */ }
#[test]
fn sidecar_shape_has_no_tauri_dependency() { /* prompt_drop compiles without any tauri import — enforced by module location */ }
```

- [ ] **Step 2: RED** → **Step 3: Implement** (wire into `wiki_context`, `wiki_search`, `curated_recall_context` result paths; emit only for non-human-tier matches) → **Step 4: Gates** (baseline + 3; `ct wisdom match` replay test green) → **Step 5: Commit** — `feat(recall): unverified-match prompt drop emitter`

### Task 7: Recall labels + confidence ordinal

**Files:**
- Modify: `src-tauri/src/wiki_graph.rs` (`WikiSearchHit` `:19-27` — add `source_type: Option<String>`, `verified: Option<VerifiedInfo>`; population where hits are built)
- Modify: `src-tauri/src/tool_dispatch.rs` (`rank_wiki_entries` `:437-443` SELECT + emit; ordinal at `:480-491` — `ORDER BY` replacement: map `confidence` via `CASE confirmed=3, inferred=2, ELSE 0` before sort)
- Test: `src-tauri/src/wiki_graph.rs` + `src-tauri/src/tool_dispatch.rs` (test modules)

**Interfaces:**
- Produces: per-hit `source_type` (from `provenance_for`, `wisdom_match.rs:264-268`, `None` outside vocab) + `verified {by, at}` on all three tools; `ct wisdom match` untouched (top-level `WikiContextResult.provenance` trace list at `wiki_graph.rs:1953` untouched).

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn hits_carry_source_type_and_verified() { /* confirmed hit has Some; inferred has None verified */ }
#[test]
fn confidence_ordinal_ranks_confirmed_above_inferred() { /* equal term overlap: confirmed ranks FIRST — the old string compare ranked it below */ }
#[test]
fn message_mode_output_byte_identical() { /* ct wisdom match fixture: cached-output replay stays green, byte compare */ }
```

- [ ] **Step 2: RED** → **Step 3: Implement** → **Step 4: Gates** (baseline + 3 + replay test) → **Step 5: Commit** — `feat(recall): per-hit provenance labels and confidence ordinal`

### Task 8: Aperture widening (#282 §5 rev-15) + Path B provenance arm

**Files:**
- Modify: `src-tauri/src/wisdom_match.rs` (`provenance_filter: Option<&[&str]>` at `:524`; predicate `:603-607` → membership test; activity arm `:762-765` → `Some(&["user_stated","user_confirmed"])`; all four `gated_entries` call sites)
- Modify: `src-tauri/src/db/commit.rs` (`resolve_proposal`: BEFORE the Task-3 arms — if `human_surface` is false AND every source document has a `stated_attestations` row with matching `content_hash` AND source count ≥ 1 → `user_stated`)
- Test: `src-tauri/src/wisdom_match.rs` + `src-tauri/src/db/commit.rs` (test modules)

**Interfaces:**
- Produces: widened activity aperture; `stated_attestations`-driven `user_stated` stamp.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn activity_admits_confirmed_and_stated() { /* user_stated + user_confirmed delivered; immutable_document NOT; NULL provenance NOT */ }
#[test]
fn non_admitted_fact_never_shadows_admitted_below() { /* non-shadowing pin, pre-truncate */ }
#[test]
fn attested_sources_stamp_user_stated() { /* all-attested → user_stated */ }
#[test]
fn mixed_or_zero_sources_stay_agent_tier() { /* mixed → librarian_inferred; zero sources → librarian_inferred */ }
#[test]
fn attestation_frontmatter_in_agent_deposit_forges_nothing() { /* deposit file WITH attestation-shaped frontmatter, no table row → librarian_inferred */ }
```

- [ ] **Step 2: RED** → **Step 3: Implement** → **Step 4: Gates** (baseline + 5; message-mode replay green) → **Step 5: Commit** — `feat(wisdom): widen activity aperture to user_confirmed and add attested-source arm`

### Task 9: App capture flow — stated/ folder, kick registration, confirm UI wiring, direct-write retirement

**Files:**
- Modify: `src-tauri/src/db/connection.rs` (`:770-773` pattern — folder-mode/ingest-tier rows for `stated/`: app-writable, agent-non-deposit; ingest tier NONE for the three `.jsonl` ledgers)
- Modify: `src-tauri/src/wisdom_deposit.rs` (`walk_vault`/`ingest_tier_for` entry for `stated/`)
- Modify: `src-tauri/src/db/wisdom.rs` (`add_wisdom_in_tx` `:127-218` REMOVED; replacement `capture_statement(app_ctx, text) -> Result<()>`: renders `stated/kurt/<year>/<slug>.md` with speaker/capture/attestation frontmatter, writes `stated_attestations` row + `.attestations.jsonl`, registers kick via `set_kick_state` `:474`, runs the kick)
- Modify: Tauri command layer (`src-tauri/src/lib.rs` commands) + frontend (`src/` React): queue screen (rows + evidence quote + aging + bulk select + denied section), confirm/deny modal, Active Verification Prompting modal (watches `.brain/unverified-match.jsonl`, renders from brain by fact id, per-drain cap 3 + digest + snooze preference)
- Test: `src-tauri/src/db/wisdom.rs` (test module)

**Interfaces:**
- Produces: `capture_statement` (Path B entry); frontend queue + modal components (contract: modal re-reads fact by id from the brain API, never renders drop-file payload text).

- [ ] **Step 1: Failing tests (backend)**

```rust
#[test]
fn capture_statement_writes_attested_file_and_kicks() { /* file exists under stated/kurt/<year>/, attestation row + jsonl line, kick ledger row, post-kick fact is user_stated with evidence */ }
#[test]
fn fresh_statement_via_queue_creates_backing_fact() { /* Rule-15: confirm-with-no-fact routes through capture_statement */ }
#[test]
fn direct_write_is_gone() { /* compile-level: add_wisdom_in_tx no longer exists (#[cfg(test)] grep guard or deletion assertion via cargo check failure expectation) */ }
```

- [ ] **Step 2: RED** → **Step 3: Implement** (backend first, then frontend components; `pnpm install --frozen-lockfile`, `pnpm build` + vitest for the queue/modal components with a mocked brain API: payload-tampering test = modal shows brain content not payload text) → **Step 4: Gates** (cargo baseline + 3; frontend suite green) → **Step 5: Commit** — `feat(app): stated capture flow, queue ui, and verification modals`

### Task 10: Charter + spec-delta text (docs task)

**Files:**
- Modify: `INTENT.md` — apply ALL amendments quoted in spec §3.10 verbatim (Rules 2/3/8 + statuses, new Rule 10, "Who verifies: users and scope", Glossary, Active Librarian planned list)
- Modify: `docs/superpowers/specs/2026-10-18-issue282-activity-query-mode-design.md` — §5 rev-15 delta (admitted set + rationale), "Out of scope" `:865-866` and "Rulings carried" `:883-885` lines
- Modify: this plan — tick annotations

- [ ] **Step 1: Apply INTENT.md amendments** (exact text from spec §3.10 — no paraphrase)
- [ ] **Step 2: Apply #282 §5 delta** (rev-15 entry: aperture admits `user_stated` + `user_confirmed`; `immutable_document` excluded; non-shadowing preserved; precondition (a) met by the kernel calibration argument + the Task 8/10 benchmark)
- [ ] **Step 3: Verify** — every quoted old passage matches what INTENT.md actually had (git diff shows pure replacements); no other INTENT edits
- [ ] **Step 4: Commit** — `docs(intent): confirmation queue, provenance labels, and user-scope documentation`

### Task 11: Kernel seeding + benchmark + whole-branch verification

**Files:**
- Create: scratch-brain seeding script (scratch only — NOT committed) confirming the 14 kernel facts as `user_confirmed` fixtures
- Modify: `src-tauri/src/wisdom_match.rs` bench module (`--features slow-tests`; `wisdom_gate_bench` needs the feature flag or it silently runs 0 tests)

- [ ] **Step 1: Benchmark** — activity FP stays 0 with the widened aperture on the seeded scratch brain (spec §3.6 by-construction argument, made empirical)
- [ ] **Step 2: Full gate sweep** — entire suite + clippy `--all-targets -- -D warnings` on touched manifests + frontend build + message-mode replay
- [ ] **Step 3: End-to-end smoke (local)** — deposit attributed note → appears in queue → confirm → `ct wisdom match --path=…` delivers it; message-mode replay byte-identical
- [ ] **Step 4: Commit** — `test(wisdom): activity fp-0 benchmark on confirmed kernel set`

## Execution notes

- Controller: run EVERY gate at baseline before Task 1 dispatch; record exact counts + the backing-service recipe (none needed — SQLite scratch brains) in `.superpowers/sdd/<plan-id>/progress.md`.
- Tasks are strictly serial (shared `wisdom_match.rs`/`commit.rs` surfaces). Task 9's frontend half may overlap Task 7's review ONLY if cargo never runs concurrently.
- On any spec/plan conflict: the spec (rev 4.1) wins; record an ERRATA blockquote in this plan.
