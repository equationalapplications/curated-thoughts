# wisdom_deposit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the approved `wisdom_deposit` tool surface (PR #258 spec): 4 new MCP/CLI tools, removal of the 4 direct-write tools + every agent-reachable approve path, folder-rule seeding, kick + catch-up sweep, docs sweep.

**Architecture:** Deposits are plain markdown documents written append-only under `immutable-source-files/agents/`; the deposit tool then runs the same per-document ingest+librarian flow the pipeline worker uses (`ingest_file` + `generate_summary`, `src-tauri/src/pipeline/mod.rs:233-246`) under `VaultLock` (held on `brain_dir`, `lib.rs:1654`), so the Librarian — the sole brain writer — mints `fact_*` rows stamped with deposit-tier provenance (`deposit_origin_tier`, `db/commit.rs:1129`). Status/pending are answered by joining `librarian_evidence → curated_proposal_sources → documents` on the deposit path — no new recall semantics.

**Tech Stack:** Rust (rusqlite, rmcp, tokio, clap), Tauri lib crate + `tools` crate CLI.

**Spec:** `docs/superpowers/specs/2026-10-01-wisdom-deposit-tool-surface-design.md` (rev 2 + Kurt review, `926f8ce`)

## Global Constraints

- Agents NEVER write brain rows directly (INTENT v2 rule 1); only file writes + the Librarian writes.
- New tool names are exactly: `wisdom_deposit`, `wisdom_deposit_status`, `wisdom_pending`, `wisdom_propose_supersession`.
- Removed names: `curated_add_wisdom`, `curated_update_wisdom`, `curated_archive_wisdom`, `curated_proposal_decide` (MCP), `ct approve` (CLI), `tools/src/bin/approve_pending_proposals.rs` (binary).
- `tools/list` stays exactly 16 names (4 out, 4 in).
- Guard prefixes extend to `wisdom_`: cloud-bridge deny (`tool_dispatch.rs:1432`) and best-effort audit exclusion (`tool_dispatch.rs:1618`) — wisdom_* write tools audit fail-closed inside their dispatchers like curated_ writes do.
- Deposits target `immutable-source-files/agents/**` ONLY, never `immutable-source-files/agents/supersessions/**` via `wisdom_deposit` (that lane belongs to `wisdom_propose_supersession`).
- Append-only: an existing target path is `deposit_exists` error; `vault_write_note` must refuse create AND If-Match edit under `immutable-source-files/agents/`.
- Folder rules seeded by migration (V25): `immutable-source-files/agents/` → `synthesize`+`auto_approve=1`; `immutable-source-files/agents/supersessions/` → `summarize`+`auto_approve=1` (explicit child override; `get_folder_mode` walks ancestors, `librarian/mod.rs:169-186`).
- Migrations: idempotent bodies; new stamp 25 gated on V22 stamped (V23/V24 pattern, `connection.rs:712-725`).
- Supersession files: `supersedes` frontmatter written by the tool; exactly-one of `target_source_ref`/`target_fact_id`; zero brain mutation.
- Live brain never touched out-of-band; tests use scratch brains; embedding stubs only in unit tests.
- CI: `cargo check --workspace`, `cargo test -p` unit suites, `CURATED_MCP_INTEGRATION_TESTS=1` integration suite locally.

---

### Task 1: V25 migration — folder-rule seeds + deposit kick-state table

**Files:**
- Modify: `src-tauri/src/db/connection.rs` (migrate(): new block after the V24 section, ~line 740)
- Test: `src-tauri/src/db/connection.rs` (tests module, near the V24 tests ~line 3050)

**Interfaces:**
- Produces: table `deposit_kick_state (path TEXT PRIMARY KEY, state TEXT NOT NULL CHECK(state IN ('started','queued_watcher','no_ingest_host','failed','ingested')), error TEXT, updated_ms INTEGER NOT NULL)`; folder_rules rows for both agent dirs. Later tasks read/write these.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn v25_seeds_agent_folder_rules_and_kick_state_table() {
    let dir = tempfile::TempDir::new().unwrap();
    // rooted open (vault_path config) so V22 stamps and 25 lands
    let conn = open_rooted_test_db(dir.path()); // same helper shape the V22 tests use
    let agents: String = conn.query_row(
        "SELECT librarian_mode || ':' || auto_approve FROM folder_rules
          WHERE folder_path = 'immutable-source-files/agents/'",
        [], |r| r.get(0)).unwrap();
    assert_eq!(agents, "synthesize:1");
    let supers: String = conn.query_row(
        "SELECT librarian_mode || ':' || auto_approve FROM folder_rules
          WHERE folder_path = 'immutable-source-files/agents/supersessions/'",
        [], |r| r.get(0)).unwrap();
    assert_eq!(supers, "summarize:1");
    // child override wins the ancestor walk
    let (mode, auto) = crate::librarian::folder_mode_for_test(
        &conn, "immutable-source-files/agents/supersessions/x.md");
    assert_eq!((mode.as_str(), auto), ("summarize", true));
}
```

(`folder_mode_for_test`: tiny `#[cfg(test)]` wrapper exposing `get_folder_mode` — or make `get_folder_mode` `pub(crate)`.)

- [ ] **Step 2: Run** `cargo test -p curated-thoughts --lib v25_seeds` → FAIL (no V25).

- [ ] **Step 3: Implement** — in `migrate()` after the V24 block:

```rust
// V25 — wisdom-deposit substrate (spec 2026-10-01-wisdom-deposit-tool-surface):
// folder rules that make agent deposits flow through the Librarian without a
// human round-trip (rule 3), and the kick-state ledger D4/D6 report from.
// Ungated idempotent data seeds + stamp gated on V22 (V23/V24 pattern).
conn.execute_batch(
    "CREATE TABLE IF NOT EXISTS deposit_kick_state (
        path       TEXT PRIMARY KEY,
        state      TEXT NOT NULL CHECK(state IN ('started','queued_watcher','no_ingest_host','failed','ingested')),
        error      TEXT,
        updated_ms INTEGER NOT NULL
    );")?;
conn.execute(
    "INSERT OR IGNORE INTO folder_rules (folder_path, librarian_mode, auto_approve)
     VALUES ('immutable-source-files/agents/', 'synthesize', 1)",
    [])?;
conn.execute(
    "INSERT OR IGNORE INTO folder_rules (folder_path, librarian_mode, auto_approve)
     VALUES ('immutable-source-files/agents/supersessions/', 'summarize', 1)",
    [])?;
if stamped >= 22 {
    conn.execute("INSERT OR IGNORE INTO schema_version (version) VALUES (25)", [])?;
}
```

- [ ] **Step 4: Run test** → PASS. Also run the V22/V23/V24 gating tests (`rootless open must not stamp...`) → still PASS.

- [ ] **Step 5: Commit** `feat(db): V25 — agent folder-rule seeds + deposit kick-state ledger`

### Task 2: Remove the four direct-write tools (MCP surface)

**Files:**
- Modify: `src-tauri/src/mcp_server.rs` (delete `#[tool]` fns `curated_add_wisdom`/`curated_update_wisdom`/`curated_archive_wisdom`/`curated_proposal_decide`, lines 219-306)
- Modify: `src-tauri/src/tool_dispatch.rs` (delete Params structs `CuratedAddWisdomParams`/`CuratedUpdateWisdomParams`/`CuratedArchiveWisdomParams`/`CuratedProposalDecideParams`; dispatch arms at 1587-1608; handlers `dispatch_curated_add_wisdom` 967, `dispatch_curated_update_wisdom`, `dispatch_curated_archive_wisdom`, `dispatch_curated_proposal_decide`)
- Modify: `src-tauri/src/lib.rs:6359-6366` (bridge-denial test list: drop the 3 removed wisdom names, keep read names)
- Modify: `src-tauri/tests/mcp_integration.rs` (16-name list at :537-560; delete the add/update round-trip block ~563+; sweep ALL FOUR removed names)
- Test: same integration file (name-list assertion is the test)

**Interfaces:**
- Produces: `tools/list` = 12 names until Task 4 adds 4 (final: 16).

- [ ] **Step 1:** Update `mcp_integration.rs` expected list FIRST to the final 16 (with the new `wisdom_*` names) but do not add new tools yet — integration test is env-gated (`CURATED_MCP_INTEGRATION_TESTS=1`) so it won't run in unit CI; the name list is finished in Task 4.
- [ ] **Step 2:** Delete the four `#[tool]` fns + Params structs + arms + handlers; fix `lib.rs` test lists (bridge test now iterates the surviving curated read tools + asserts `wisdom_deposit` is denied too — updated again in Task 7 if needed).
- [ ] **Step 3:** `cargo check -p curated-thoughts --lib --features mcp-server` → compiles; `grep -rn "curated_add_wisdom\|curated_update_wisdom\|curated_archive_wisdom\|curated_proposal_decide" src-tauri/src tools/src` → only spec/docs/historical-comment hits + `db/commit.rs:56` doc comment (update that comment to `wisdom_deposit`).
- [ ] **Step 4:** `cargo test -p curated-thoughts --lib` → PASS.
- [ ] **Step 5: Commit** `feat(mcp)!: remove direct-write wisdom tools (INTENT v2 rule 1)`

### Task 3: wisdom core module — deposit/status/pending/supersession (lib side)

**Files:**
- Create: `src-tauri/src/wisdom_deposit.rs` (core fns; pub via `lib.rs` re-export like `ingest_document_with_vault_root`)
- Modify: `src-tauri/src/lib.rs` (module decl + re-exports)
- Modify: `src-tauri/src/vault/config.rs` / `okf/write.rs` writable-subdir logic: `NOTE_WRITABLE_SUBDIRS` (`vault/safe_path.rs:50`) loses `AGENTS_DEPOSIT_DIR` — `vault_write_note` must refuse `immutable-source-files/agents/**` entirely (create + If-Match)
- Test: `src-tauri/src/wisdom_deposit.rs` unit tests (scratch brain + temp vault)

**Interfaces (exact signatures downstream tasks use):**

```rust
pub struct DepositOutcome { pub path: String, pub pending: bool,
    pub kick: &'static str } // "started" | "queued_watcher" | "no_ingest_host"
pub fn wisdom_deposit_on(conn: &mut Connection, vault_root: &Path,
    db_path: &Path, title: &str, body: &str, tags: &[String])
    -> Result<DepositOutcome>;
// writes <slug>.md via safe_vault_path(MayCreate) restricted to AGENTS_DEPOSIT_PREFIX
// minus supersessions/; records deposit_kick_state; kicks (below); upserts state row.

pub enum DepositStatus { Pending { kick: &'static str }, Chunked,
    Ingested { fact_ids: Vec<String>, tier: Option<String> }, Failed { error: String } }
pub fn wisdom_deposit_status_on(conn: &Connection, path: &str) -> Result<Option<DepositStatus>>;
// probe: SELECT e.proposal_id, e.entry_id FROM librarian_evidence e
//        JOIN curated_proposal_sources s ON s.proposal_id = e.proposal_id
//        JOIN documents d ON d.id = s.doc_id WHERE d.path = ?1 LIMIT 1
// plus chunks existence (documents/chunks by path) for Chunked; kick_state for Failed.

pub fn wisdom_pending_on(conn: &Connection, vault_root: &Path)
    -> Result<Vec<(String, i64, &'static str)>>; // (path, mtime, kick)

pub struct SupersessionOutcome { pub path: String }
pub fn wisdom_propose_supersession_on(conn: &Connection, vault_root: &Path,
    target_source_ref: Option<&str>, target_fact_id: Option<&str>,
    replacement_title: &str, replacement_body: &str, reason: &str)
    -> Result<SupersessionOutcome>;
// exactly-one target (bail otherwise); target_source_ref must match ^librarian-[0-9a-f]{32}$
// (bundle_apply.rs:186 shape) when given; fact_id target resolves to its source_ref via
// llm_wiki_entries; writes supersessions/<ts>-<slug>.md with supersedes: <token> frontmatter
// (plain text frontmatter, NOT parse_fact_file — deposits are ordinary documents)
```

Kick body (inside `wisdom_deposit_on`, after the file write), mirroring `pipeline/mod.rs:233-246`:

```rust
enum Kick { Started, WatcherHolds, NoHost }
fn run_kick(db_path: &Path, vault_root: &Path, rel_path: &str) -> Kick {
    // VaultLock is held on brain_dir (lib.rs:1654), so contention == a running watcher
    match crate::watcher::VaultLock::acquire(&db_path.parent().unwrap().to_path_buf()) {
        Err(_) => return Kick::WatcherHolds,          // queued_watcher
        Ok(_lock) => { /* fall through and run */ }
    }
    // endpoints configured?
    let paths = crate::retrieval::brain_paths_for(&db_path.parent().unwrap().to_path_buf());
    let Ok(report) = BrainConfig::load_lenient(&paths) else { return Kick::NoHost };
    let Some(profile) = report.config.embed_profile else { return Kick::NoHost };
    let mut conn = match Connection::open(db_path) { Ok(c) => c, Err(_) => return Kick::NoHost };
    let _ = conn.busy_timeout(Duration::from_secs(5));
    let ingest = pipeline::ingest_document_with_vault_root(
        &conn, &profile, rel_path, false, Some(vault_root.to_str().unwrap()));
    let librarian = ingest.and_then(|_| librarian::generate_summary(
        &mut conn_since_mut_wrapper, &rel_path,
        &librarian::active_generation_model(crate::setup::recommended_model()), false));
    // state row: ingested on Ok, failed w/ error text on Err (both via UPSERT)
}
```

Called via `tokio::task::spawn_blocking` from the dispatcher (Task 4); the lib-side fn is synchronous so `ct` can call it too. `vault_write_note` refusal: remove `AGENTS_DEPOSIT_DIR` from `NOTE_WRITABLE_SUBDIRS` and add an explicit `is_deposit_path(path)` → `DepositAreaReadOnly` error in `okf/write.rs::write_note` (both create and If-Match paths).

- [ ] **Step 1: failing tests** (unit, `wisdom_deposit.rs` tests module with `seed_file_db`-style scratch brain + temp vault + stub/None embed profile):
  1. deposit writes file under agents/ with expected title/body/tags text and `pending: true`, kick `started` (embed stub configured) — then run librarian with a stub completer? NO: unit test stops at file+state-row level; full-pipeline assertion lives in the integration test (Task 7).
  2. deposit outside agents/ errors; deposit INTO supersessions/ errors; deposit to existing path errors (`deposit_exists`).
  3. kick with no embed profile → state `no_ingest_host`, still `pending: true`, file exists.
  4. `wisdom_deposit_status_on`: no rows → `Pending`; `documents` row without librarian_evidence → `Chunked`; after inserting a proposal+evidence+sources fixture (shape from `db/proposals.rs` fixtures) → `Ingested{fact_ids}`.
  5. supersession: exactly-one enforcement; `supersedes:` line in file; zero brain row count change.
  6. `vault_write_note` under agents/ → refusal (create + If-Match).
- [ ] **Step 2:** run → FAIL. **Step 3:** implement. **Step 4:** run → PASS + `cargo test -p curated-thoughts --lib`. **Step 5: Commit** `feat(wisdom): deposit/status/pending/supersession core + agents/** write lockout`

### Task 4: MCP dispatchers + tool registrations

**Files:**
- Modify: `src-tauri/src/tool_dispatch.rs` (4 new Params structs + arms + async dispatchers; guard prefixes at 1432 & 1618 gain `|| tool.starts_with("wisdom_")`; the fail-closed `log_agent_access_checked` write-audit inside each wisdom write dispatcher)
- Modify: `src-tauri/src/mcp_server.rs` (4 `#[tool]` registrations where the old ones sat)
- Modify: `src-tauri/tests/mcp_integration.rs` (final 16-name list; deposit round-trip test)
- Modify: `src-tauri/src/lib.rs` bridge test (wisdom_deposit denied for bridge sessions)

Params (serde, `#[serde(default)]` options, schemars derive matching siblings):

```rust
pub struct WisdomDepositParams { pub title: String, pub body: String, #[serde(default)] pub tags: Vec<String> }
pub struct WisdomDepositStatusParams { pub path: String }
pub struct WisdomPendingParams {}
pub struct WisdomProposeSupersessionParams { #[serde(default)] pub target_source_ref: Option<String>,
  #[serde(default)] pub target_fact_id: Option<String>, pub replacement_title: String,
  pub replacement_body: String, pub reason: String }
```

Returns: deposit `{path, pending, kick}`; status `{path, status: "pending"|"chunked"|"ingested"|"failed", fact_ids?, tier?, error?}`; pending `{pending: [...]}`; supersession `{path}`. Dispatchers call the Task-3 fns inside `ctx.with_rw` (write ops) / read conn (status/pending); deposit's kick runs via `spawn_blocking` after the with_rw block; write audit `log_agent_access_checked(..., "write")` inside the RW tx for deposit + supersession (fail-closed), reads via `log_curated_access_rw`-style.

- [ ] Steps: write integration-test deposit round-trip FIRST (env-gated): deposit → file exists → status eventually `ingested` with `fact_ids` non-empty and no human-tier label (brain needs real embeddings: skip-if-no-Ollama pattern from CTI e2e `1284746`). Then implement, `cargo check --workspace`, unit tests green. **Commit** `feat(mcp): wisdom_deposit tool surface (16-name MCP wire)`

### Task 5: CLI — `ct wisdom`, remove `ct approve`, TTY gate, binary deletion

**Files:**
- Modify: `tools/src/bin/ct.rs` (`Cmd` enum: remove `Approve` variant + `approve_cmd` 548; add `Wisdom { #[command(subcommand)] cmd: WisdomCmd }` with `Deposit{title,body,tags,--yes,json}`, `Status{path,--json}`, `Pending{--json}`, `ProposeSupersession{...--yes,--json}`)
- Modify: `tools/src/cmds.rs` (delete `approve_one`/`approve_all`/`approve_one_on`/`approve_all_on` + the "Approve" section; `proposals_review_cmd` gains the TTY gate)
- Delete: `tools/src/bin/approve_pending_proposals.rs`
- Test: `tools/` unit tests + bin-count assert lives in integration (Task 7)

TTY gate (top of `proposals_review_cmd`, `cmds.rs:1426`):

```rust
use std::io::IsTerminal;
if !std::io::stdin().is_terminal() {
    anyhow::bail!("refusing: the human verification gate requires an interactive terminal \
                   (piped/PTY approval is not human approval)");
}
```

- [ ] Steps: TTY-gate test first (assert the bail by calling the fn with piped stdin under `#[cfg(unix)]`... simplest: extract `ensure_interactive()` helper and unit-test that it errors when `stdin().is_terminal()` is false — in CI it always is, so invert: test the helper's error text via a `#[cfg(test)]` force flag). Implement; delete binary + approve fns; `cargo build --workspace` (16→15 bin files); grep dead refs. **Commit** `feat(cli)!: ct wisdom subcommands; close agent-reachable approve paths`

### Task 6: catch-up sweep + docs sweep

**Files:**
- Modify: `tools/src/cmds.rs` `watch_run` startup (after lock acquisition, before event loop): scan `vault/immutable-source-files/agents/**.md` (skip `supersessions/`), for each file with no librarian_evidence probe hit and kick-state != ingested → run the Task-3 kick fn. Start-only, logged to stderr.
- Modify: `tools/src/bin/curated_thoughts_mcp.rs:584` (instructions line 6 → `wisdom_deposit`)
- Modify: `.skills/curated-thoughts/skill.md`, `README.md` (tool list sections), `CHANGELOG.md`
- Test: sweep covered by integration test 4 (Task 7)

- [ ] Steps: implement sweep; flip docs; `grep -rn "curated_add_wisdom" README.md .skills/` → clean. **Commit** `feat(watch): start-only catch-up sweep for stranded deposits; docs sweep`

### Task 7: integration tests + guard tests (CI-gated §2 list)

**Files:**
- Modify: `src-tauri/tests/mcp_integration.rs` (16 names; deposit round-trip w/ real embeddings skip-if-unconfigured; lock contention → `queued_watcher`; supersession file shape; bridge denial for `wisdom_deposit`; bin-count assert `std::env::var("CARGO_BIN_DIR")` listing == 15 or assert `approve_pending_proposals` absent via `CARGO` metadata at test runtime — simplest: `assert!(!tools_dir.join("approve_pending_proposals").exists())` against `env!("CARGO_TARGET_TMPDIR")`-adjacent bin dir via `CARGO_BIN_EXE_` absence is not testable; instead assert `ct approve` errors "unknown command" by running the built ct binary)
- Create: `src-tauri/tests/wisdom_deposit_integration.rs` (tests 3/4/8: kick semantics incl. concurrent-lock `queued_watcher`, catch-up sweep picks up a dropped file on `ct watch --once` startup, folder-rule override: librarian run over a supersession deposit produces zero fact rows)

- [ ] Steps: write tests; run full local suite `cargo test --workspace` + `CURATED_MCP_INTEGRATION_TESTS=1 cargo test -p curated-thoughts --test mcp_integration --test wisdom_deposit_integration` (skip-branch if no embedding backend); fix fallout. **Commit** `test(wisdom): spec §2 CI-gated suite`

### Task 8: PR + dual review

- [ ] Push branch `feat/wisdom-deposit-impl`; open PR against main referencing #258; run GLM self-review + Opus independent review (same mechanics as the spec round: pinned-blob evidence, verdict JSON to `~/.local/state/opus-review/`); apply findings; CI green; park for Kurt merge approval per delivery flow.
