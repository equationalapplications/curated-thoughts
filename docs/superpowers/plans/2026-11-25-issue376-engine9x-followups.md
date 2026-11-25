# Issue #376 engine-9.x follow-ups — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the vault-switch residue leaks (stated-ledger tables, `deposit_kick_state`), fix the wrong-root reopen, gate helper-SQL drift, and land the issue's hygiene items — per the approved spec `specs/engine9x-followups/spec.md` (owner rulings Q5–Q9, 2026-11-25) and the converged investigation `specs/engine9x-followups/investigation.md`.

**Architecture:** the sweep (`clear_vault_tables`) gains DELETEs inside its existing transaction (rebuild stamp untouched); `switch_vault` writes the config before the post-sweep reopen so every root-dependent step sees the NEW vault; the pipeline worker gains a drain hook that runs the confirmations leg when the queue drains; the TS schema test gains an engine-ownership-filtered normalized-SQL diff; a new FTS-carrying fixture proves the desktop upgrade path.

**Tech Stack:** Rust (rusqlite, tauri), TypeScript (vitest, node:sqlite), GitHub Actions.

**Spec:** `specs/engine9x-followups/spec.md` (this plan argues from it; executors read both).

## Global Constraints

- Base branch `chore/engine9x-followups` (PR #379). Plans ride the PR; pruned at merge.
- Line numbers in this plan refer to base `10e9b006`; re-anchor with grep before editing.
- Rust gates run ONLY via the heavy-build wrapper with `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`; `/tmp` is a 3.6G tmpfs — never put cargo target dirs there. First gate is a cold rebuild (~15–25 min).
- CI runs clippy with `-D warnings` on both manifests; local chains MUST include it (`src-tauri` adds `--features test-utils,mcp-server`).
- `llm_wiki_meta` and its `stated_ledger_rederived` stamp are NEVER deleted by any task. `llm_wiki_outbox` is append-only. `llm_wiki_vector_removals` stays out of every sweep list.
- No pin bumps, no engine upgrades, no changes to `package_ddl_objects`.
- Full test output goes to a file; pinned counts compared with awk (`test result:` lines), never `| tail`.

---

### Task 0: Plan-time verification checks (no code)

**Files:** none (checks only; results recorded in this file's checkboxes).

- [x] **Step 1: Enumerate rust-side-only `llm_wiki_*` index/trigger objects** (S8 allow-list completeness):

```bash
cd /mnt/kv-work/build/worktrees/curated-thoughts/engine9x-followups
# Rust-chain brain: build one via the existing in-memory helpers in a quick test run
CT_DUMP_RUST=$(mktemp -p /mnt/kv-work/scratch/tmp)
CT_DUMP_ENG=$(mktemp -p /mnt/kv-work/scratch/tmp)
CT_REGEN_FIXTURES=1 cargo test --manifest-path src-tauri/Cargo.toml --lib \
  --features test-utils engine9_migrated_schema_fixture_is_current 2>&1 | tail -3
# engine-only brain: fresh node:sqlite + setup()
node --experimental-sqlite -e "
const {DatabaseSync}=require('node:sqlite');
const {engine}=require('./node_modules/@equationalapplications/core-llm-wiki');
const db=new DatabaseSync('/mnt/kv-work/scratch/tmp/eng-only.db');
await engine(db).setup();
" 2>/dev/null || node -e "
const {DatabaseSync}=require('node:sqlite');
const {engine}=require('./node_modules/@equationalapplications/core-llm-wiki');
const db=new DatabaseSync('/mnt/kv-work/scratch/tmp/eng-only.db');
engine(db).setup();
"
sqlite3 src-tauri/src/db/testdata/ct-engine9-migrated.llm_wiki.schema.sql "" # sanity: file parses
# compare name sets (the test itself will do this in-suite; this step just records the allow-list)
echo "record: rust-only llm_wiki objects -> S8 CT_OWNED allow-list"
```

Expected: the only rust-side-only `llm_wiki_*` index/trigger objects are the `llm_wiki_vector_removals*` family (pending index, enqueue + hard-delete triggers). If MORE appear, add them to the S8 allow-list with a one-line reason each.

- [x] **Step 2: Fixture feasibility probe (S9)** — install 8.0.2 in a scratch dir and prove it can produce an FTS-carrying brain:

```bash
mkdir -p /mnt/kv-work/scratch/tmp/ct376-fixture && cd /mnt/kv-work/scratch/tmp/ct376-fixture
npm init -y >/dev/null 2>&1
npm install @equationalapplications/core-llm-wiki@8.0.2 >/dev/null 2>&1
node -e "
const {DatabaseSync}=require('node:sqlite');
const {engine}=require('@equationalapplications/core-llm-wiki');
const db=new DatabaseSync('./brain.db');
await engine(db).setup();
db.exec(\"INSERT INTO llm_wiki_entries (entity_id, title, body, source_type, confidence) VALUES ('ent_x','T','FTS probe body text','user_stated',100)\");
console.log('fts objects:', db.prepare(\"SELECT COUNT(*) n FROM sqlite_master WHERE name LIKE '%fts%'\").get().n);
"
```

Expected: `fts objects: > 0`. If 8.0.2's API differs (no `engine(db)` export), read its `dist/index.js` export surface and adapt the harness — record the working snippet; Task 10 depends on it. If generation is NOT feasible in ≤30 min of work, STOP and split S9 to its own issue per the spec's Q5 fallback.

- [x] **Step 3: Q6 join-back check (spec S7)** — read the engine search flow in the installed dist (`bm25` MATCH queries at `dist:7076-7078`, `:7517-7519`) far enough to answer: can a stale FTS hit surface a deleted entry's text? Record YES (→ open the follow-up issue per Q6) or NO (→ record; S7 stands as documented). This is a bounded read, not a spike.

- [x] **Step 4: Commit** (nothing to commit; record all three results in this file by ticking the boxes and noting deviations).

> **Task 0 results (controller, 2026-11-25):** Step 1 — CT_OWNED allow-list grows 3→8
> (`vector_removals_resurrect` EXISTS in the rust chain; `idx_source_passages_doc/entity`,
> `entries_deleted_idx`, `events_forgotten_idx` are CT-owned too — full reasons in the SDD
> ledger). Engine-side exclusions needed: FTS family (rust chains never create FTS) and
> `entries_legacy_source_type_idx` (dump's source brain was Rust-built; real 8.0.2 desktop
> brains carry it) — Task 9 implements per this finding. Step 2 — FEASIBLE in-lane
> (<10 min): 8.0.2 exports `createWiki` only (no `engine`), needs a SQLiteAdapter over
> node:sqlite + async setup + `syncSearchIndex()` drain; ERRATA E3/E4 apply to all
> engine-invocation snippets. Step 3 — join-back answer NO: `searchKeyword` hits hydrate
> via `entryRepo.findByIds` only; `searchPassages` join-back-guards on `source_passages`
> — S7 stands as documented, NO follow-up issue.

---

### Task 1: S1 — migrate repair reports off stdout

**Files:**
- Modify: `src-tauri/src/db/connection.rs:541`, `:694`, `:918` (three `println!(` → `eprintln!(`; text unchanged)

- [x] **Step 1: Verify the three sites are still exactly these** (re-anchor):

```bash
grep -n '\bprintln!(' src-tauri/src/db/connection.rs
```

Expected: exactly lines 541, 694, 918.

- [x] **Step 2: Edit** — change `println!(` to `eprintln!(` at each of the three sites. Do not touch the message strings.

- [x] **Step 3: Verify**:

```bash
grep -n '\bprintln!(' src-tauri/src/db/connection.rs   # expect: no output
grep -c 'eprintln!' src-tauri/src/db/connection.rs     # ERRATA E2: expect 22 (base had 19) — verified 22
```

- [x] **Step 4: Fast check + commit**:

```bash
export CARGO_TARGET_DIR=$PWD/.cargo-shared-target
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
cargo check --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server -j 2 2>&1 | tail -3
git add src-tauri/src/db/connection.rs
git commit -m "fix(db): migrate repair reports to stderr — stdout is the --mcp JSON-RPC channel (#376)"
```

---

### Task 2: S2 — `engine9_missing` groups PRAGMA reads by table

**Files:**
- Modify: `src-tauri/src/db/connection.rs:1494-1514` (`engine9_missing`)
- Test: `src-tauri/src/db/connection.rs` (existing `engine9` test module; the `:8040`-area test already covers coverage)

**Interfaces:**
- Produces: `fn grouped_mirror_tables() -> Vec<&'static str>` — distinct tables from `ENGINE9_MIRROR_COLUMNS` in first-appearance order; consumed only inside `engine9_missing`.

- [x] **Step 1: Add the helper + rewrite the loop** in `connection.rs` (just above `engine9_missing`):

> **Task 1+2 closeout (controller, 2026-11-25):** implementer commit `e2a490e9` + `50849454`;
> controller re-ran the engine9 gate (12 passed, exit 0) and verified both clippy exits via the
> child's report at the same HEAD. Ruling: plan snippet's `t == table` needed a `*t == table`
> deref for the `&&str`/`&str` comparison — accepted, mechanical type fix, behavior identical
> (12 passed unchanged, 1723 filtered). Note: branch upstream is origin/main — push with
> `git push origin HEAD:chore/engine9x-followups`.

```rust
/// Distinct tables in ENGINE9_MIRROR_COLUMNS, first-appearance order.
/// One PRAGMA table_info per DISTINCT table instead of per column.
fn grouped_mirror_tables() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for (table, _, _) in ENGINE9_MIRROR_COLUMNS {
        if !out.contains(table) {
            out.push(table);
        }
    }
    out
}
```

In `engine9_missing`, replace the per-column loop:

```rust
    let mut columns = Vec::new();
    for table in grouped_mirror_tables() {
        let existing = crate::db::ddl_compat::existing_columns(conn, table)?;
        for (t, column, _) in ENGINE9_MIRROR_COLUMNS {
            if t == table && !existing.is_empty() && !existing.iter().any(|c| c == column) {
                columns.push(format!("{table}.{column}"));
            }
        }
    }
```

- [x] **Step 2: Run the affected tests** (expect unchanged pass — column coverage identical):

```bash
cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server \
  --lib engine9 -j 2 2>&1 | tee /mnt/kv-work/scratch/tmp/t2.log | grep "test result"
```

Expected: all `engine9*` tests pass, same counts as base (record them).

- [x] **Step 3: Commit**:

```bash
git add src-tauri/src/db/connection.rs
git commit -m "perf(db): engine9_missing reads PRAGMA table_info once per table (#376)"
```

---

### Task 3: S3 — stale CI sqlite comments ×3

**Files:**
- Modify: `.github/workflows/ci.yml:223-225`, `.github/workflows/ci.yml:135`, `scripts/runtime-smoke-clw.mjs:18`

- [x] **Step 1: Rewrite the three comments** to: "node:sqlite shipped behind `--experimental-sqlite` in 22.5.0 and was unflagged in 22.13.0 (nodejs/node#55890); the flag is a harmless no-op on current 22.x and kept to avoid churning a working invocation." In `ci.yml:135`, additionally correct ">= 22.5, built-in sqlite" → ">= 22.13 unflagged; shipped behind the flag in 22.5".

- [x] **Step 2: Verify**:

```bash
grep -n "22.13" .github/workflows/ci.yml scripts/runtime-smoke-clw.mjs
```

Expected: three matches.

- [x] **Step 3: Commit**:

```bash
git add .github/workflows/ci.yml scripts/runtime-smoke-clw.mjs
git commit -m "docs(ci): correct node:sqlite flag history comments (unflagged in 22.13.0) (#376)"
```

> **Tasks 3+4 closeout (controller, 2026-11-25):** commits `1e28cbf5` + `0658ef0d`; the
> plan's "expect: three matches" check reads FOUR grep lines — site 2 (ci.yml:135) carries
> BOTH the corrected parenthetical (":135") and the full history comment (":136"). Intentional
> per the plan's site-2 instruction; verified controller-side. `git check-attr diff` on the .db
> reports `diff: unset` = the `binary` macro's `-diff` — correct no-diff behavior.

---

### Task 4: S4 — scoped `.gitattributes`

**Files:**
- Create: `.gitattributes` (repo root)

- [x] **Step 1: Create** `.gitattributes`:

```
# Byte-compared fixtures must not be re-normalized on Windows checkouts
# (connection.rs engine9_migrated_schema_fixture_is_current compares on_disk == dump).
src-tauri/src/db/testdata/*.sql text eol=lf
src-tauri/src/db/testdata/*.db binary
```

- [x] **Step 2: Verify attributes apply and no churn**:

```bash
git check-attr text eol -- src-tauri/src/db/testdata/ct-engine9-migrated.llm_wiki.schema.sql
git check-attr text diff -- src-tauri/src/db/testdata/ct-brain-v32-core-8.0.2.db
git ls-files --eol src-tauri/src/db/testdata/ | head
```

Expected: schema.sql → `text: set, eol: lf`; brain.db → `binary: -diff` (or `diff: binary`); all tracked testdata files already `w/i/lf` (no CRLF in repo — if any shows `w/crlf`, note it: working-tree re-checkout needed on Windows only).

- [x] **Step 3: Append the re-checkout note** to `src-tauri/src/db/testdata/README.md`:

```markdown
## Line endings

`testdata/*.sql` is pinned to LF via `.gitattributes` (the Rust drift gate
byte-compares it). Windows clones created before that attribute landed need
one re-checkout: `git rm --cached -r src-tauri/src/db/testdata && git reset --hard`.
```

- [x] **Step 4: Commit**:

```bash
git add .gitattributes src-tauri/src/db/testdata/README.md
git commit -m "chore: pin testdata fixtures to LF — byte-compare is autocrlf-fragile (#376)"
```

---

### Task 5: S5a — sweep DELETEs (TDD)

**Files:**
- Modify: `src-tauri/src/db/queries.rs` (the `execute_batch` inside `clear_vault_tables`, `:258-303`)
- Test: `src-tauri/src/db/queries.rs` (existing `#[cfg(test)]` module, helpers `count()` `:745`, `seed_full_vault()` `:755`)

**Interfaces:**
- Consumes: `open_in_memory` (`crate::db::connection`), existing `count` helper.
- Produces: sweep covers `llm_wiki_source_documents`, `llm_wiki_source_passages`, `stated_attestations`, `queue_denials`, `deposit_attribution`, `deposit_kick_state`. The `stated_ledger_rederived` row in `llm_wiki_meta` MUST survive (asserted).

- [x] **Step 1: Write the failing tests** — append to the test module in `queries.rs`:

> **Task 5 closeout (controller, 2026-11-25):** implementer BLOCKED honestly on a REAL plan
> defect — `deposit_kick_state` is V25 root-gated (parks on rootless opens, `open_in_memory`
> migrates rootless) so the plain DELETE breaks 4 rootless sweep tests and the RED seed.
> RULING: existence-guard ONLY the kick_state DELETE (a brain without the table never had a
> rooted session — skipping is a semantic no-op); other five DELETEs stay plain (every-open
> DDL); test seeds V25's exact shape first. Implemented on the SAME session `d4d00fcb`;
> GREEN 7 passed (controller re-run), clippy ×2 exit 0, origin read back. RED caveat: the
> captured RED fails on the kick_state assertion (WIP carried the other five DELETEs when
> re-run); the pristine stated_attestations-first RED was not re-captured — accepted,
> mechanism proven.

```rust
    #[test]
    fn clear_vault_tables_empties_stated_ledger_and_kick_state_but_keeps_stamp() {
        let mut conn = open_in_memory().unwrap();
        seed_full_vault(&conn);
        // Seed every newly-swept table (schema exists via the every-open chain).
        conn.execute(
            "INSERT INTO deposit_kick_state (path, state, updated_ms)
             VALUES ('docs/a.md', 'ingested', 1)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO deposit_attribution (document_path, speaker, kind, source, deposited_at)
             VALUES ('docs/a.md', 'spk', 'kind', 'src', 1)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO stated_attestations (document_path, content_hash, attested_by, attested_at)
             VALUES ('docs/a.md', 'h', 'by', 'at')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO queue_denials (document_path, chunk_hash, fact_text_hash)
             VALUES ('docs/a.md', 'ch', 'fh')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_source_documents (entity_id, source_ref, source_hash)
             SELECT 'ent_x', 'ref', 'hash'
             WHERE EXISTS (SELECT 1 FROM sqlite_master WHERE name='llm_wiki_source_documents')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO llm_wiki_meta (key, value) VALUES ('stated_ledger_rederived', '1')",
            [],
        ).unwrap();

        super::clear_vault_tables(&mut conn, 1).unwrap();

        for table in [
            "stated_attestations",
            "queue_denials",
            "deposit_attribution",
            "deposit_kick_state",
            "llm_wiki_source_documents",
            "llm_wiki_source_passages",
        ] {
            assert_eq!(count(&conn, table), 0, "{table} must be swept");
        }
        // Stamp survives — the once-per-brain rederive mechanism is untouched.
        let stamp: String = conn
            .query_row(
                "SELECT value FROM llm_wiki_meta WHERE key='stated_ledger_rederived'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stamp, "1");
        // Outbox still append-only: replica DELETEs for entries were pushed.
        assert!(count(&conn, "llm_wiki_outbox") > 0);
    }
```

(Adjust the `deposit_attribution`/`stated_attestations`/`queue_denials` INSERT column lists to the exact schemas in `src-tauri/src/db/migrations/0001_deposit_attribution.sql:13` and `0002_stated_verification.sql:12/:19` — read them first; the shapes above are from the migration bodies.)

- [x] **Step 2: Run — expect FAIL** (tables not swept):

```bash
cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server \
  --lib clear_vault_tables -j 2 2>&1 | tee /mnt/kv-work/scratch/tmp/t5.log | grep -E "test result|panicked"
```

Expected: new test FAILS on `stated_attestations must be swept`.

- [x] **Step 3: Implement** (with the kick_state existence-guard ruling) — inside `clear_vault_tables`'s `execute_batch` (children-before-parents order is free; no FKs), after the existing DELETEs and before the `llm_wiki_meta` cleanup block:

```sql
         DELETE FROM stated_attestations;
         DELETE FROM queue_denials;
         DELETE FROM deposit_attribution;
         DELETE FROM deposit_kick_state;
         DELETE FROM llm_wiki_source_documents;
         DELETE FROM llm_wiki_source_passages;
```

Add a comment block above them: per-vault rule #213 — these are vault-derived
(document_path / vault-relative keys) and must not outlive the vault; the
`stated_ledger_rederived` stamp survives (repopulation is switch-level, spec §2 S5).
Also add the S7 doc comment on `pub fn clear_vault_tables` (above the fn): the
raw `DELETE FROM llm_wiki_entries` leaves vault A's FTS shadow text at rest
until the engine's next drain (`*_fts_ad` only enqueues into `*_fts_pending`,
dist-verified); accepted as at-rest-only for CT (owner Q6 ruling 2026-11-25;
production Rust search never reads FTS) — engine-owned boundary per #213.

- [x] **Step 4: Run — expect PASS** (same command as Step 2). All pre-existing `clear_vault_tables*` tests must stay green. VERIFIED: 7 passed / 0 failed.

- [x] **Step 5: Commit**:

```bash
git add src-tauri/src/db/queries.rs
git commit -m "fix(db): sweep the stated-ledger, kick-state and source-store tables on vault switch (#376, #213)"
```

---

### Task 6: S5b — config write before the reopen + switch-level pin tests (TDD)

**Files:**
- Modify: `src-tauri/src/lib.rs:2296-2330` (in-place branch + restore branch tail + shared reopen block)
- Test: new `src-tauri/src/db/switch_pin_tests.rs` (registered in `src-tauri/src/db/mod.rs` under `#[cfg(test)]`) — a seam, see below

**Interfaces:**
- Produces: `pub(crate) fn switch_reopen_paths(db_path: &Path, config_path: &Path) -> Result<AppDb>` — extracted from the `:2305-2310` reopen block so tests can drive the SAME order the switch uses (extract, don't duplicate).
- Produces: `pub fn apply_stored_confirmations(conn: &Connection, vault_root: &Path) -> Result<usize>` in `stated_ledger.rs` (Task 7 consumes).

Reality note: `switch_vault` is a tauri-command-sized function with app state; there are NO existing switch_vault tests (verified). The pin tests therefore drive the extracted seam against real temp vaults — no Tauri State involved.

- [ ] **Step 1: Write the failing pin tests** (`src-tauri/src/db/switch_pin_tests.rs`). Harness: tempdir with vault A (`stated/.confirmations.jsonl` + `stated/.attestations.jsonl` + `deposits/` with one deposit file + `.brain/brain.db` created via `AppDb::open_with_config`), vault B likewise; config names A; run the sweep; run the NEW order; assert:

```rust
//! Switch-level pin tests (spec §2 S5). These drive the extracted reopen seam
//! against real temp vaults — the tauri command itself stays unexercised.
use super::*;
use crate::db::connection::AppDb;

fn make_vault(root: &Path, name: &str) -> PathBuf {
    let stated = root.join("stated");
    std::fs::create_dir_all(&stated).unwrap();
    std::fs::write(
        stated.join(".confirmations.jsonl"),
        format!("{{\"fact_id\":\"f_1\",\"content_hash\":\"h_{name}\",\"by\":\"kurt\",\"at\":\"t\"}}\n"),
    )
    .unwrap();
    root.to_path_buf()
}

#[test]
fn inplace_switch_rederives_from_new_vault_and_keeps_stamp() {
    // 1. Brain + config name vault A; open once so stamps land.
    // 2. Run clear_vault_tables on a raw connection (the in-place branch body).
    // 3. NEW ORDER: set_vault_path(B) BEFORE AppDb::open_with_config.
    // 4. Assert: the three stated-ledger tables hold ONLY B-derived rows
    //    (A's document paths absent), the stamp value is unchanged, and
    //    B's ledger rows are present (run-1 rebuild executed with B's root).
}
```

(Full assertions mirror spec §2 S5 Pin: tables hold B's rows; stamp value
unchanged; the run-1 `rebuild_rederive` used `roots.canonical` = B — assert
via a `deposit_attribution` row whose document_path resolves under B.)

- [ ] **Step 2: Run — expect FAIL** (config still names A at reopen; rederive writes A rows).

- [ ] **Step 3: Implement the reorder** in `switch_vault`:
  - In-place branch (`:2296-2302`): after `clear_vault_tables` succeeds, call `vault_state.0.lock().unwrap().set_vault_path(&new_path)`; set `pending_config_align_to = None` (NOT `Some`).
  - Restore branch: after `clear_install_marker(&db_path)` (`:2291`), same two lines.
  - Remove the later `set_vault_path` + `pending_config_align_to = None` pair (`:2325-2332`) — it becomes redundant; keep the `vault-switched` emit and the rest.
  - Keep the error-path align block (`:2373-2379`) as a safety net (now normally a no-op).
  - Immediately after the reopen block (`AppDb::open_with_config` at `:2307`), add Run 1: if the sweep branch ran, call `crate::db::stated_ledger::rebuild_rederive(conn, &new_root.canonical)` on the reopened connection and log the report via `eprintln!` (NOT stdout).

- [ ] **Step 4: Run — expect PASS.** All `restore_sync` and existing lib tests stay green.

- [x] **Step 5: Commit**:

```bash
git add src-tauri/src/lib.rs src-tauri/src/db/switch_pin_tests.rs src-tauri/src/db/mod.rs
git commit -m "fix(vault): write config before the post-sweep reopen — root-dependent steps see the NEW vault (#376)"
```

---

### Task 7: S5c — repopulation: run-1 wiring + run-2 drain hook (TDD)

**Files:**
- Modify: `src-tauri/src/db/stated_ledger.rs` (new pub fn, after `rebuild_rederive` `:356`)
- Modify: `src-tauri/src/pipeline/mod.rs` (worker loop `:186-200` + `PipelineWorker` field)
- Test: `src-tauri/src/db/stated_ledger.rs` tests + `src-tauri/src/pipeline/mod.rs` tests

**Interfaces:**
- Produces: `pub fn apply_stored_confirmations(conn: &Connection, vault_root: &Path) -> Result<usize>` — reads `stated/.confirmations.jsonl`, runs `apply_confirmations_by_hash` over it, returns applied count. (Run 2 uses this; `rebuild_rederive` is NOT re-entered.)
- Produces: worker field `confirmations_hook: Option<(Arc<PathBuf-pair>, Arc<AtomicBool>)>` — set via `new_with_vault` callers; simpler concrete shape: `stated_confirmations: Option<Arc<dyn Fn(&Connection) -> Result<()> + Send>>` installed by `start_pipeline` when a switch requested it (set ONCE per epoch by switch_vault via the handle before respawn; cleared after first fire). Keep the concrete shape simple and local.

- [ ] **Step 1: Write the failing test** for `apply_stored_confirmations`:

```rust
    #[test]
    fn apply_stored_confirmations_stamps_matching_entries_without_touching_other_legs() {
        // build brain + vault root with a .confirmations.jsonl line whose
        // content_hash matches a seeded entry's title+body hash
        // (reuse the hashing helper the rebuild tests use)
        let applied = apply_stored_confirmations(&conn, &vault_root).unwrap();
        assert_eq!(applied, 1);
        assert_eq!(count(&conn, "stated_attestations"), 0, "table legs untouched");
    }
```

- [ ] **Step 2: Implement** `apply_stored_confirmations` — mirror rebuild step 1 (`:375-381`): read `CONFIRMATIONS_LEDGER`, open `unchecked_transaction`, `apply_confirmations_by_hash(&tx, &lines)`, commit, return applied.

- [ ] **Step 3: Write the failing worker test** — construct a `PipelineWorker` via `new_with_vault` with an in-memory channel pair, install the hook closure counting invocations, drive one `Ingest` job to completion with `pending` reaching 0 and an empty channel; assert the hook fired exactly once and `pending`-nonzero crossings don't fire it.

- [ ] **Step 4: Implement the drain hook** in the worker loop: after the counted-job decrement, when `pending.load() == 0`, replace the next blocking `self.rx.recv()` with `self.rx.recv_timeout(DRAIN_PROBE)` (const `DRAIN_PROBE: Duration = Duration::from_millis(250)`):
  - `Err(RecvTimeoutError::Timeout)` → channel empty during the probe → fire the installed hook once (on a connection opened the same way `run_inner` opens it), clear it, continue the loop with blocking recv.
  - `Ok(job)` → keep it as `next_job` and loop (no fire).
- [ ] **Step 5: Wire the install** — `switch_vault`'s in-place branch sets the hook on the fresh `PipelineHandle` right after `start_pipeline` (`:2321-2322`), closing over the new vault root; `start_pipeline` itself stays unchanged.
- [ ] **Step 6: Run all pipeline + stated_ledger tests — expect PASS** (record pinned counts).
- [ ] **Step 7: Commit**:

```bash
git add src-tauri/src/db/stated_ledger.rs src-tauri/src/pipeline/mod.rs
git commit -m "fix(pipeline): stated-ledger repopulation — rebuild at switch, confirmations on queue drain (#376)"
```

---

### Task 8: S6 — brain-wide sweep guard test

**Files:**
- Test: `src-tauri/src/db/queries.rs` test module (new test)

**Interfaces:**
- Consumes: the static disposition const defined in the test; `sqlite_master` of an `open_in_memory` brain seeded via `seed_full_vault`.

- [ ] **Step 1: Write the guard test** (this one is the deliverable — write it first, it should PASS only after the dispositions list is complete; if it fails on Day 1 that IS the point):

```rust
    /// Brain-wide guard: every table must be swept, partially-cleared-with-
    /// reason, or exempted-with-reason (spec §2 S6). Static list, cross-checked
    /// against sqlite_master of a seeded in-memory brain.
    #[test]
    fn every_brain_table_has_an_explicit_switch_disposition() {
        let conn = open_in_memory().unwrap();
        seed_full_vault(&conn);
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        // FTS objects are engine-owned and absent from Rust in-memory brains;
        // documented + enforced in the TS-side inventory (spec S7/S8).
        const SWEPT: &[&str] = &[
            "llm_wiki_entries", "llm_wiki_tasks", "llm_wiki_edges", "llm_wiki_events",
            "llm_wiki_source_ref_index", "llm_wiki_checkpoints", "curated_agent_log",
            "entity_type_origin", "stated_attestations", "queue_denials",
            "deposit_attribution", "deposit_kick_state",
            "llm_wiki_source_documents", "llm_wiki_source_passages",
            // …+ every remaining table the sweep deletes — enumerate from
            // queries.rs:214-303 and keep in sync; the asserts below enforce it.
        ];
        const PARTIAL: &[&str] = &["llm_wiki_entity_manifests"];
        const EXEMPT: &[&str] = &[
            "llm_wiki_meta",              // survives INTACT (incl. the rederive stamp)
            "llm_wiki_outbox",            // append-only replica ledger
            "llm_wiki_vector_removals",   // #213/T9 marker-gated
            "schema_version",             // watermark
            "system_strikes",             // vault-independent single-row ledger
            "pipeline_heartbeat",         // operator-facing (subject note: spec Q9)
            "pipeline_stalls",
            "folder_rules", "documents", "wiki_pages", // swept — re-list from :214-303
        ];
        for t in &tables {
            assert!(
                SWEPT.contains(&t.as_str()) || PARTIAL.contains(&t.as_str())
                    || EXEMPT.contains(&t.as_str()),
                "table {t} has no switch disposition — sweep it or add it with a reason (spec §2 S6)"
            );
        }
        // Negative control: a future table fails this test by construction.
        assert!(SWEPT.contains(&"llm_wiki_entries"));
    }
```

- [ ] **Step 2: Reconcile the list against the ACTUAL sweep** (`queries.rs:214-303`) — every table in the sweep goes under SWEPT; resolve mismatches by fixing the LIST, not the sweep. Also seed any table the default `open_in_memory` chain creates but `seed_full_vault` doesn't (the sqlite_master query must see them — if a table only exists post-V33 on real brains, seed it explicitly or use the V33 fixture brain helper).
- [ ] **Step 3: Run — expect PASS** with the final list.
- [x] **Step 4: Commit**:

```bash
git add src-tauri/src/db/queries.rs
git commit -m "test(db): brain-wide guard — every table has an explicit vault-switch disposition (#376)"
```

---

### Task 9: S8 — TS normalized-SQL drift gate with engine-ownership filter

**Files:**
- Test/Modify: `src/__tests__/engine9-setup-compat.test.ts` (new `it(...)` in the existing describe)

**Interfaces:**
- Consumes: `rustChainDb()`, `engine(db).setup()`, `schemaObjects` (existing); Task 0 Step 1's allow-list.

- [ ] **Step 1: Write the test** (append to the describe block):

```typescript
  function normalizedSql(db: DatabaseSync, kind: string): Map<string, string> {
    const rows = db.prepare(
      "SELECT name, sql FROM sqlite_master WHERE type = ? AND name NOT LIKE 'sqlite_%'",
    ).all(kind) as { name: string; sql: string }[];
    return new Map(rows.map((r) => [r.name, (r.sql ?? "").replace(/\s+/g, " ").trim()]));
  }

  // CT_OWNED: rust-side llm_wiki_* objects absent from a fresh engine brain.
  // Populated from Task 0 Step 1's enumeration; an unlisted rust-only object
  // FAILS (the rustOnly assert below).
  const CT_OWNED = new Set([
    "index:llm_wiki_vector_removals_pending",
    "trigger:llm_wiki_vector_removals_enqueue",
    "trigger:llm_wiki_vector_removals_hard_delete",
  ]);

  it("engine-owned index/trigger SQL bodies match a fresh engine brain (normalized)", async () => {
    const db = rustChainDb();
    db.prepare("INSERT INTO llm_wiki_meta (key, value) VALUES ('schema_version', ?)")
      .run(ENGINE_8_0_2_SCHEMA_VERSION);
    const fresh = new DatabaseSync(":memory:");
    await engine(fresh).setup();

    const fail: string[] = [];
    for (const kind of ["index", "trigger"] as const) {
      const rust = normalizedSql(db, kind);
      const eng = normalizedSql(fresh, kind);
      for (const [name, sql] of eng) {
        const got = rust.get(name);
        if (got === undefined) fail.push(`${kind} ${name}: MISSING from rust chain`);
        else if (got !== sql) fail.push(`${kind} ${name}: SQL drift\n  rust:  ${got}\n  engine: ${sql}`);
      }
      for (const [name] of rust) {
        const key = `${kind}:${name}`;
        if (!eng.has(name) && name.startsWith("llm_wiki_") && !CT_OWNED.has(key)) {
          fail.push(`${kind} ${name}: rust-only llm_wiki object not on the CT_OWNED allow-list`);
        }
      }
    }
    expect(fail).toEqual([]);
  });
```

- [ ] **Step 2: Run**:

```bash
pnpm vitest run src/__tests__/engine9-setup-compat.test.ts 2>&1 | tail -15
```

Expected: PASS (Task 0 Step 1 guaranteed the allow-list is complete). If the v18/v19 partial indexes are MISSING from the engine brain (migration-only, replay-dependent), the plan-time check records it and this test gains the dedicated assertion for them — implement per that finding.

- [ ] **Step 3: Negative control (temporary, local-only)**: perturb one mirror SQL in `okf_ddl.rs`, run, expect the drift FAIL, revert. Record output.

- [x] **Step 4: Commit**:

```bash
git add src/__tests__/engine9-setup-compat.test.ts
git commit -m "test(ts): helper-SQL drift gate — normalized body diff vs fresh engine brain, engine-ownership filter (#376)"
```

---

### Task 10: S9 — FTS-carrying fixture + desktop-upgrade tests

**Files:**
- Create: `src-tauri/src/db/testdata/ct-brain-v33-src-8.0.2-fts.db` (generated, committed), README entry
- Create: `src/__tests__/engine9-upgrade-fixture.test.ts` (or extend the compat test file)
- Test: `src-tauri/src/db/connection.rs` test module (V33-over-populated-tables)

- [ ] **Step 1: Generate the fixture** using the Task 0 Step 2 harness (full setup + several entities' writes so FTS carries text), then `VACUUM INTO` a clean copy. Hygiene: `strings` over the file — 0 local-path hits; README provenance paragraph (how/when generated, engine versions, regen command).
- [ ] **Step 2: TS test** — copy of the compat test's first case over the new fixture: stamp `schema_version=15`, run 9.18.0 `setup()`, assert version + `dropped == []` modulo the FTS family already present (assert the FTS objects survive setup unchanged).
- [ ] **Step 3: Rust test** — open the fixture brain in temp, run the V33 chain (`apply_engine9_mirror`), assert the guarded ALTERs no-op on existing columns and data survives (`SELECT COUNT(*) FROM llm_wiki_entries` unchanged).
- [ ] **Step 4: Run both, commit**:

```bash
git add src-tauri/src/db/testdata/ src/__tests__/ src-tauri/src/db/connection.rs
git commit -m "test: desktop-upgrade path over an FTS-carrying 8.0.2 brain (#376)"
```

---

### Task 11: Gates + wrap

- [ ] **Step 1: Full wrapped gate chain** (mount guard first; ~cold):

```bash
mountpoint -q /mnt/kv-work && [ "$(df --output=avail -BG /mnt/kv-work | tail -1 | tr -dc 0-9)" -ge 30 ] \
  || { echo 'kv-work not mounted or <30G free'; exit 3; }
~/.hermes/bin/heavy-build --need-mb 3500 --mem-high 4500 --mem-max 6000 -- bash -c '
  export CARGO_TARGET_DIR=/mnt/kv-work/build/cargo/curated-thoughts
  export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
  cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
  cargo clippy --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server --all-targets -- -D warnings
  cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server -j 2 -- --test-threads=1
  cargo clippy --manifest-path tools/Cargo.toml --all-targets -- -D warnings
  cargo test --manifest-path tools/Cargo.toml -- --test-threads=1
' 2>&1 | tee /mnt/kv-work/scratch/tmp/t11-gates.log
awk '/test result:/' /mnt/kv-work/scratch/tmp/t11-gates.log
```

- [ ] **Step 2: Vitest + tsc** (`pnpm test`, `pnpm tsc --noEmit` or the repo's script) — green.
- [ ] **Step 3: Post-run sweep** — prune the debuginfo-era artifacts if any mixed-profile dirs appeared; `df -h /mnt/kv-work` recorded.
- [ ] **Step 4: Push; ladder review (Opus → Fable) on the PR; CI green; owner go → merge-commit ONLY; prune plan + spec per convention at the cleanup commit; records to the vault.**
