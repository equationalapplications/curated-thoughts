# `areas` Implementation Plan (spec 2026-11-14-areas, rev 11) — plan rev 4 (tier-3 rounds 1-3 folded; round-4 rails on this rev)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Repo-scoped statement delivery — the `area` column end to end: migration, model write path, the IOC owning-hub carrier on deposits, the three-arm SCOPED predicate, per-item band/area output, the rename command, and the measured attach rate.

**Architecture:** CT-local `area` TEXT column on `llm_wiki_entries` (V30); the Librarian stamps it from a per-hub prompt clause with the owning hub force-included as a candidate via the deposit's `hub:` frontmatter (written at deposit time by the shared resolver); delivery composes temporal eligibility with a NEW `band_predicate_where` three-arm function inside `gated_entries` only; renames run through the outbox with merge-group expansion. UNSCOPED paths stay byte-identical.

**Tech Stack:** Rust (src-tauri lib crate + standalone `tools/` crate), rusqlite, `globset` (NEW dep, §9.3), serde_yaml for areas files.

**Spec:** `specs/2026-11-14-areas/spec.md` (rev 11, Reviewed). The plan argues from the spec; executors read both.

## Global Constraints

- CT IS a cargo workspace (root `Cargo.toml`: members `src-tauri`, `tools`) — GATE COMMAND MATRIX (tier-3 r1 MAJOR-1 + r2 MAJOR-1/3, both rails): unit tests `cargo --manifest-path src-tauri/Cargo.toml test --lib --features test-utils`; integration tests `cargo --manifest-path src-tauri/Cargo.toml test --test <name> --features test-utils` (CREATE the file before the run or the target does not exist); tools-side tests `cargo --manifest-path tools/Cargo.toml test --test <name>` — tools has NO `[features]` section, never pass `--features test-utils` there. `cargo test --lib` runs ONLY unit tests inside `src/` — it can never execute a `tests/<name>.rs` file, so any "expect FAIL→PASS" cycle on an integration test must use `--test <name>`. "Full suite" = BOTH targets per crate. Never chain a gate through a pipe; serial runs per the disk rules.
- UNSCOPED output must stay byte-identical: the message-mode pin (`tool_dispatch.rs:2195`) and the four activity pins (`tools/tests/ct_wisdom_match.rs:359/:378/:409/:427`) must pass unchanged (§19.1).
- Bands are LABELS ONLY (M4): no ranking change anywhere in this plan.
- No backfill (Q9/#343): legacy rows keep `area = NULL`.
- Bundles keep dropping `area`; only the per-import summary line is added (§8.4).
- `two_stage.rs` is NOT modified (§9.2, §9.5); `wisdom_match_with_floor` (`wisdom_match.rs:378`) is NOT modified (§17); `tool_dispatch.rs` change is the pin-comment update only (§10.4).
- Stage-1 stays brain-wide in SCOPED — ACCEPTED, do not "fix" (§9.2).
- Conventional commits; `type(scope)!: subject` form only if breaking (none expected). Frequent commits, one per task minimum.
- `cargo clippy` gate is "no NEW findings" (pre-existing debt is out of scope; verify base before claiming).
- New `tests/`-visible helpers default to `pub`, not `pub(crate)` (integration-test visibility).

---

### Task 1: Migration V30 + INSERT gains `area` (§5)

**Files:**
- Modify: `src-tauri/src/db/connection.rs` (V30 wiring beside V27–V29; V27 `:829`, V28 `:1116`, V29 `:1321`)
- Modify: `src-tauri/src/db/commit.rs:1697-1702` (new-fact INSERT gains the column + bind)
- Test: `src-tauri/tests/db_migration_v30_area.rs` (new)

**Interfaces:**
- Produces: `llm_wiki_entries.area TEXT NULL` in every fresh and migrated brain; the INSERT path accepts `area: Option<&str>`.
- Consumes: nothing new.

- [ ] **Step 1: Write the failing migration test** — open a temp brain via the repo's standard test fixture, assert `PRAGMA table_info(llm_wiki_entries)` contains `area` with type TEXT and default NULL, on BOTH a fresh brain and a MIGRATED brain: seed a brain stamped at V29 — the runner reads `COALESCE(MAX(version),0) FROM schema_version` (`db/connection.rs:228`) and the pub entry is `migrate_open_db` (`:1603`) — so insert a `schema_version` row set at V29 with a V29-shaped `llm_wiki_entries`, run `migrate_open_db`, assert the column (tier-3 r2 m5). A fresh-brain-only assertion proves nothing about the migration path (tier-3 r1 m1). Run via `--test db_migration_v30_area`, not `--lib`.
- [ ] **Step 2: Run** `cargo --manifest-path src-tauri/Cargo.toml test --test db_migration_v30_area --features test-utils` — expect FAIL (no V30).
- [ ] **Step 3: Implement V30** — one `ALTER TABLE llm_wiki_entries ADD COLUMN area TEXT NULL;` wired beside V29. Follow the `embed_scheme` precedent (`db/connection.rs:1065`).
- [ ] **Step 4: Extend the new-fact INSERT** (`db/commit.rs:1697-1702`) with the `area` column and bind parameter; thread an `area: Option<&str>` argument from the caller; existing callers pass `None`.
- [ ] **Step 5: Run the commit/db tests** — `cargo --manifest-path src-tauri/Cargo.toml test --lib --features test-utils` — expect PASS (existing tests unchanged; NULL path preserved).
- [ ] **Step 6: Commit** `feat(db): V30 adds llm_wiki_entries.area; INSERT binds it`

### Task 2: `WisdomConfig` — the `wisdom` object in config.json (§3.3)

**Files:**
- Modify: `src-tauri/src/config/mod.rs` (beside `BrainConfig`; `config.json` is JSON, not TOML — gate-4 m1)
- Test: unit tests in the same file's `#[cfg(test)]` module or `src-tauri/tests/wisdom_config.rs`

**Interfaces:**
- Produces:
  - `pub struct WisdomConfig { pub areas_paths: Vec<(String, PathBuf)>, pub org_entity_id: Option<String> }` — `areas_paths` keyed by repo entity id, value = absolute areas-file path (§3.3).
  - `impl WisdomConfig { pub fn from_value(v: &serde_json::Value) -> Result<Option<WisdomConfig>> }` — absent `wisdom` object → `Ok(None)`.
  - Duplicate-directory rejection: two entries whose areas-file PARENT DIRECTORY (the checkout root, §3.1) is equal → hard config error at load (§3.3, the tie-impossibility guarantee) — compared on CANONICALIZED absolute paths, so symlinks, trailing slashes, and case cannot slip past the guard (tier-3 r1 n3).

- [ ] **Step 1: Write failing tests** — parse a `wisdom` object with two entries; assert fields; assert duplicate-directory load error; assert absent object → None; assert `org_entity_id` round-trip.
- [ ] **Step 2: Run** `cargo --manifest-path src-tauri/Cargo.toml test --lib --features test-utils wisdom_config` — expect FAIL.
- [ ] **Step 3: Implement** `WisdomConfig` + wire it into the config loader so `config.json`'s `wisdom` object populates it (parse site: `BrainConfig::load` at `config/mod.rs:1301`; mirror the per-block lenient `from_value` family at `:1609-1817` — tier-3 r2 m2; `librarian/mod.rs:182` reads the config beside the brain — mirror that derivation).
- [ ] **Step 4: Run full lib suite** — PASS.
- [ ] **Step 5: Commit** `feat(config): wisdom.areas_paths + org_entity_id (WisdomConfig)`

### Task 3: The shared resolver module (§9.3 core + §6.8.2)

**Files:**
- Create: `src-tauri/src/config/hub_resolver.rs` (+ `pub mod hub_resolver;` in `config/mod.rs`)
- Test: `src-tauri/tests/hub_resolver.rs`

**Interfaces:**
- Consumes: `WisdomConfig` (Task 2); `resolve_redirect_chain` (`db/merge_duplicates.rs:447`, `pub`), `cluster_ids` (`db/entities.rs:371`, `pub(crate)` — the resolver is in-crate, fine), `live_entities` view (`db/schema.rs:584-589`).
- Produces:
  - `pub enum DepositHub { Unconfigured, NoPrefix, Unresolved, Hub(String) }` (§6.8.2/§6.8.5 reasons `wisdom_unconfigured` / `no_prefix` / `unresolved`).
  - `pub fn resolve_deposit_hub(conn: &Connection, cfg: Option<&WisdomConfig>, repo_path: &str) -> DepositHub` — takes `&str`: `absent` (the PARAM itself missing) is the DOOR's translation before the resolver is ever invoked (Task 8, tier-3 r1 n1/r2 m3); no config → `Unconfigured`; canonicalize via longest-existing-ancestor + remainder (§9.3 rule); longest-prefix match over canonicalized checkout roots; resolve matched id through `resolve_redirect_chain` (`None` → own id, `Survivor(s)` → `s`, `Cycle` → `Unresolved`); live check on the survivor; same-survivor collision between two configured keys → `Unresolved` + one loud stderr line (§3.3, gate-5 C2).
  - `pub struct ScopedMatch { pub repo_group: Vec<String>, pub org_group: Option<Vec<String>>, pub area: Option<String> }`.
  - `pub fn resolve_scoped(conn: &Connection, cfg: &WisdomConfig, resolved_path: &str) -> Option<ScopedMatch>` — CLI `--path=` side: resolve-to-survivor FIRST then `cluster_ids(survivor)` expansion (gate-6 m3); `area` = the area name whose pattern matches, matched with `globset` on the path relative to the checkout root; `org_group` from `org_entity_id` resolved + expanded; `None` when no root prefixes (query stays byte-identical, §9.8). Org omission is NEVER silent: an org id that fails resolve+live emits ONE stderr line at match start (the `print_gate_decision` house form, `tools/src/queries.rs:1056`) and the org arm is omitted (§9.3 — tier-3 r1 MAJOR-3; assert the line in a test).
  - `pub fn resolve_hub_id(conn: &Connection, cfg: &WisdomConfig, id: &str) -> DepositHub` — the ID-based rule for the Librarian read side (§6.8.4): resolve `id` through `resolve_redirect_chain`, live-check the survivor, accept ONLY when the survivor equals a CONFIGURED `wisdom.areas_paths` key's survivor (tier-3 r2 GLM m3 — one home for the rule, consumed by Task 6; prevents deposit-time and Librarian-time answers diverging).
  - Areas-file loader (tier-3 r1 MAJOR-2 — §3.1–§3.5): `pub struct AreasFile { pub areas: Vec<(String, Vec<String>)> }` (name → glob patterns) + an mtime-gated cache — ONE cache IMPLEMENTATION, instantiated per process (the match-time reader is the one-shot `ct`; the Librarian reader is the sidecar/app — tier-3 r2 m4), re-read ONLY when the file's mtime changed, used by BOTH readers. Malformed file → error, no partial state.
  - `pub fn canonicalize_lenient(path: &str) -> String` — longest EXISTING ancestor + re-appended remainder (§9.3, gate-3 m1). Shared by both sides.
- Dep: `globset` added to `src-tauri/Cargo.toml` in this task (§9.3 — plan budgets it here, per spec).

- [ ] **Step 1: Write failing tests** — fixture brain with two entities + config; cases: innermost-checkout nesting wins; symlinked-cwd canonicalization; non-existent `--path` ancestor+remainder; no prefix → `None`/`NoPrefix`; `repo_path` relative is the CALLER's refusal (not tested here); redirect chain: merged-away configured key still binds via resolve-then-expand; cycle → `Unresolved`; same-survivor collision → `Unresolved` + stderr line; glob `infra/**` matches only relative-to-root; org id unresolvable → org arm omitted + ONE stderr line (§9.3); areas-file loader: valid parse, malformed → error no partial state, mtime change between two reads forces re-read (tier-3 r1 MAJOR-2/3).
- [ ] **Step 2: Run** `cargo --manifest-path src-tauri/Cargo.toml test --test hub_resolver --features test-utils` — expect FAIL (module absent).
- [ ] **Step 3: Implement** the module (canonicalize-lenient helper first, then prefix match, then resolve-to-survivor + `cluster_ids` expansion, then live check, then glob matching via `globset`).
- [ ] **Step 4: Run** the resolver integration target (`--test hub_resolver`) + full unit suite (`--lib`) — PASS.
- [ ] **Step 5: Commit** `feat(config): shared hub resolver (longest-prefix, redirect+live, glob match)`

### Task 4: `band_predicate_where` + `gated_entries`/`WisdomItem` + bindings plumbing (§9, §10)

**Files:**
- Modify: `src-tauri/src/wisdom_match.rs` (NEW `band_predicate_where`; `gated_entries` `:520`/SELECT `:541-547`; `CandidateRow` `:445`; `WisdomItem` `:134-143`; entry points `:1228`; private chain `:905`/`:949`/`:973`/`:423`)
- Modify: `tools/src/queries.rs:898`/`:1105` (entry-point calls pass bindings)
- Modify: `src-tauri/src/tool_dispatch.rs:2195` (pin COMMENT update only, §10.4 — tier-3 r1 m3: no task owned this edit)
- Test: band-predicate UNIT tests in `wisdom_match.rs`'s existing `mod tests` (`gated_entries` is private; precedent `:2109`, `:2161` — tier-3 r2 m5). (No Task-4 additions to `tools/tests/` — the SCOPED CLI tests are Task 5's, tier-3 r2 n1.)

**Interfaces:**
- Consumes: `ScopedMatch` (Task 3).
- Produces:
  - `pub struct BandBindings<'a> { pub repo_group: &'a [String], pub org_group: Option<&'a [String]>, pub area: Option<&'a str> }`
  - `pub fn band_predicate_where(bindings: &BandBindings) -> String` — the ONLY home of the three arms (§9.2): area arm `e.entity_id IN (…) AND e.area = ?`, repo arm `… AND e.area IS NULL`, org arm on `org_group AND e.area IS NULL`; area arm OMITTED when `area` is None (§9.4).
  - `gated_entries` gains `bindings: Option<&BandBindings>` — composed after `entry_eligibility_where`; `CandidateRow` gains `area: Option<String>`; `WisdomItem` gains `pub band: Option<String>, pub area: Option<String>` with `#[serde(skip_serializing_if = "Option::is_none")]` (§10.2); band assigned from the bound group the row's `entity_id` falls in (§17, gate-5 m3).
  - `wisdom_match_in_scheme` (DEFINITION at `wisdom_match.rs:336` — `queries.rs:1098` is a caller, tier-3 r2 m1) and `wisdom_match_two_stage_with_v1` (`:1228`) gain `bindings: Option<&BandBindings>` (UNSCOPED = `None`); the org arm is OMITTED when `org_group` is `None` — the same omission rule as the area arm (§9.4; an emitted arm with an empty bind is a syntax error or a silent zero, tier-3 r2 GLM m1) — and a negative test asserts it; `wisdom_match_with_floor` (`:378`) UNTOUCHED — VERIFIED its body does not call `wisdom_match_in_scheme`, so no `None`-passing is needed there (§17, gate-7 m2).
- Budgeted fallout: `WisdomItem` derives `PartialEq` (`:134`) — every struct-literal fixture gains the two `None` fields (mechanical; §10.7).

- [ ] **Step 1: Write failing predicate tests** — three arms bind correctly; area arm omitted when area is None; cross-repo same-name areas do NOT cross-match (§9.1); merge-group membership: facts on survivor AND loser ids both match (§9.2, seeds rows on both ids).
- [ ] **Step 2: Run** `cargo --manifest-path src-tauri/Cargo.toml test --lib --features test-utils band_predicate` — expect FAIL (`band_predicate_where` undefined).
- [ ] **Step 3: Implement** `band_predicate_where` + `gated_entries` bindings + `WisdomItem` fields + plumbing. `entry_eligibility_where` and its other consumers stay untouched (§9.2). UNSCOPED SQL byte-identical.
- [ ] **Step 4: Fix fixture fallout** — update every `WisdomItem` struct literal with the two new fields.
- [ ] **Step 5: Write the §9 guard tests** — corrections are band-independent (`find_corrections`, `:1368-1398`, pin per §9.6); provenance-blind: two fixtures differing only in `source_type` → identical SCOPED output (§9.7). (The §9.8 outside-every-checkout pin lives ONLY in Task 5 — single owner, tier-3 r1 n2.) Add the §10.4 pin-comment update to `tool_dispatch.rs:2195` in this task's implementation step.
- [ ] **Step 6: Run** — src-tauri: `--lib` + any `--test` targets created so far; tools: `test --test ct_wisdom_match` (NO `--features` — tier-3 r2 both-rails MAJOR). PASS, including the four activity pins and the message-mode byte pin UNCHANGED (the pins are exercised by the tools integration target, never by `--lib`).
- [ ] **Step 7: Commit** `feat(match): band_predicate_where three-arm SCOPED predicate; WisdomItem band/area`

### Task 5: SCOPED CLI wiring (§9.3 remainder)

**Files:**
- Modify: `tools/src/queries.rs:824` (`wisdom_match_cmd`): `--path=` → resolve against cwd → `resolve_scoped` → `ScopedMatch` → `BandBindings` → SCOPED call; no prefix → byte-identical UNSCOPED call (§9.8)
- Modify: `tools/src/bin/ct.rs:852-865` — `--path=` still passed verbatim; the resolver is a new layer (§9.3)
- Test: `tools/tests/ct_wisdom_match.rs`

**Interfaces:** Consumes `resolve_scoped` (Task 3), `band_predicate_where` plumbing (Task 4).

- [ ] **Step 1: Write failing tests** — SCOPED query with an areas file returns only matching-area + whole-repo facts of the bound group; no `--path` → UNSCOPED bytes unchanged; `--path` outside every checkout → UNSCOPED bytes unchanged (the NEW pin, §9.8); §4.4 no-pattern case: `--path` INSIDE a configured checkout matching NO glob → area `None`, area arm omitted, repo+org bands only (tier-3 r2 n2).
- [ ] **Step 2: Run** `cargo --manifest-path tools/Cargo.toml test --test ct_wisdom_match` (NO `--features`) — expect FAIL.
- [ ] **Step 3: Implement** the wiring (CLI only; no sidecar SCOPED surface — §9.3).
- [ ] **Step 4: Run** `cargo --manifest-path tools/Cargo.toml test --test ct_wisdom_match --test cli_wisdom_two_stage` (NO `--features` — tools has none) — PASS, including the four activity pins (`:359/:378/:409/:427`; the `:378` two-stage pin is CLI-level) and the message-mode byte pin unchanged (tier-3 r2 BOTH-RAILS MAJOR: `--lib --features test-utils` here is doubly impossible — no features section, and `--lib` never runs `tests/` targets).
- [ ] **Step 5: Commit** `feat(cli): ct wisdom match --path= SCOPED wiring`

### Task 6: Librarian write path — hub read, force-include, areas clause, `LlmFact.area`, drops (§6)

**Files:**
- Modify: `src-tauri/src/librarian/mod.rs` (hub read at ingest, reader precedent `:251-260`/`:320-324`)
- Modify: `src-tauri/src/db/commit.rs:1616` (`commit_fact_add` — thread the parsed `LlmFact.area` into the new-fact INSERT bind; tier-3 r2 GLM MAJOR-1: without this owner, layer 3 "the commit sets the column" fails with no one to fix it)
- Modify: `src-tauri/src/librarian/synthesis.rs` (`LlmFact` `:91-103` + payload `:947-951` + schema string `:622`; per-hub clause in prompt builder `:660-698`; force-include rank-1 before truncate `:489-522`; validate-pass drops `:901-903` region; §6.5 `new_name` drop; §6.6 area rejection; attach clause `:899-917`)
- Test: `src-tauri/tests/areas_write_path.rs` (the three-layer acceptance test, §7)

**Interfaces:**
- Consumes: resolver (Task 3 — `resolve_hub_id` for the Librarian-side id rule, the survivor rule living in the one shared module), `WisdomConfig`, and the Task 3 `AreasFile` CACHED LOADER — the per-hub areas clause (§6.3), the §6.5 union-of-names NOCASE drop, and the §6.6 vocabulary check read configured names THROUGH the mtime-gated cached loader, NEVER a one-shot `serde_yaml` read inside `synthesis.rs` (§3.5 "applies to both readers"; tier-3 r3 m2).
- Produces: prompt carries per-hub areas clauses (`For <hub entity id>: areas = …`, one line per configured hub IN the candidate set); owning hub force-included at rank 1 when the deposit file carries a valid `hub:`; `LlmFact.area` parses and rides `fact_add` payloads; `new_name`-equals-area-name edges drop into `dropped_edges` with a reason; out-of-vocabulary `area` proposals rejected and reported (§6.6); attach clause added (§11.1).

- [ ] **Step 1: Write the failing tests** (§7 + §6.5/§6.6/§6.8.4 negatives — tier-3 r1 m2/MAJOR-4): (i) per-hub clause form in the built prompt (NOT a merged list), with the owning hub present via force-include; §6.3(c) negative case targets a NON-owning configured hub and asserts the REPORT; (ii) `area` on a model fact parses into `fact_add`; (iii) commit sets the column (layer 3 column-half asserted HERE; the payload-half assertion is written in Task 7 — tier-3 r3 MAJOR-1); (d) §6.5 negative: an edge item whose `new_name` equals (NOCASE) a configured area name lands in `dropped_edges` with a reason; (e) §6.6 negative: a fact `area` not defined by the matched hub's file is rejected and reported; (f) hand-authored `hub:` negative: a `hub:` resolving to a live NON-configured entity is treated `unresolved` — no force-include, §11.2 bucket increments; (g) TRUNCATION ORDER: a fixture seeding ≥9 ranked candidates must still contain the force-included owning hub in the final batch — a truncate-before-insert implementation would drop it and every small-fixture test would stay green (§6.3a's normative order; tier-3 r3 m1). The fixtures HAND-WRITE the `hub:` frontmatter (legal per §6.8.4 — the render that writes it is Task 8; tier-3 r2 m4).
- [ ] **Step 2: Run** `cargo --manifest-path src-tauri/Cargo.toml test --test areas_write_path --features test-utils` — expect FAIL.
- [ ] **Step 3: Implement** the hub read (frontmatter map read of the deposit file, degrade → `absent`; survivor-equality check against CONFIGURED `wisdom.areas_paths` keys enforced HERE — §6.8.4, tier-3 r1 MAJOR-4), the force-include (dedupe + move to rank 1, THEN truncate to `MAX_CANDIDATES = 8`), clause scoping to in-candidate hubs keyed by RESOLVED id, the §6.3(c) reported drop, `LlmFact.area` + payload + schema string, the §6.5 drop, the §6.6 rejection, and the attach clause.
- [ ] **Step 4: Run** the `areas_write_path` integration target (`--test areas_write_path --features test-utils`) + full unit suite (`--lib`) — PASS. **Layer-3 scope (tier-3 r3 MAJOR-1):** the COLUMN half ("the commit sets the column" — read `llm_wiki_entries.area` back after the fact-add commit) goes GREEN HERE, because the `commit_fact_add` bind (`:1616`; its payload call site `:1740` sits inside it) is in this task's files; the PAYLOAD half ("the outbox payload carries it") stays RED until Task 7 changes `wiki_fact_outbox_payload`'s signature and its `:1740`/`:1960` call sites — Task 7's Step 4 is where layer 3 completes end to end.
- [ ] **Step 5: Commit** `feat(librarian): areas write path — hub read, force-include, per-hub clause, LlmFact.area, drops`

### Task 7: Outbox payload + schema guard + bundle summary line (§8)

**Files:**
- Modify: `src-tauri/src/db/commit.rs:1249` (`wiki_fact_outbox_payload` gains `area: Option<&str>`) + call sites `:1740`, `:1960`
- Modify: `src-tauri/src/db/wisdom.rs:499`, `db/drafts.rs:69`, `db/restore_sync.rs:596` (+ its SELECT `:536-541` gains `area`), `db/bundle_apply.rs:517` (pass NULL) — SIX call sites total (§8.2)
- Modify: `src-tauri/src/db/schema_guard.rs:53-56` (`area` listed CT-local beside `tier`/`embed_scheme`)
- Modify: `db/bundle_apply.rs` — the per-import summary line `N facts imported; bundles carry no area; all land whole-repo` (§8.4)
- Test: `src-tauri/tests/areas_outbox.rs`

**Interfaces:** Consumes Task 1's column; the rename task (Task 9) consumes this payload builder. NOTE: `wiki_fact_outbox_payload` (`:1249`) and `push_entries_outbox` (`:1342`) are `pub(crate)` — the integration test asserts through the PUB commit path and reads `outbox` table rows directly; the payload-SHAPE assertion lives in `commit.rs`'s `mod tests` (tier-3 r2 m2).

- [ ] **Step 1: Write failing tests** — payload shape (unit, in `commit.rs` `mod tests`: the built payload JSON carries `area`); integration (via the pub commit path: `outbox` table rows carry the area in their payload JSON); round trip through `sync_batch` passes it unchanged; sink receives `OutboxEvent.payload` containing it (§8.5); bundle import prints the summary line exactly once per import.
- [ ] **Step 2: Run** `cargo --manifest-path src-tauri/Cargo.toml test --test areas_outbox --features test-utils` — expect FAIL.
- [ ] **Step 3: Implement** all six call sites + guard line + summary line.
- [ ] **Step 4: Run** full lib suite — PASS (three-layer test's layer 3 now completes end to end).
- [ ] **Step 5: Commit** `feat(outbox): area rides wiki_fact payloads; schema guard + bundle summary line`

### Task 8: The carrier — `repo_path`, render, reply contract (§6.8)

**Files:**
- Modify: `src-tauri/src/wisdom_deposit.rs` (`repo_path` param `#[serde(default)]`; door checks `:930-940` region; render `:177-208` — attribution lines first, `hub:` LAST, byte-identical when both absent (existing pin); reply `hub`/`hub_reason`; status fn `dispatch_wisdom_deposit_status` at `:1058` (form helpers `:261-290` — gate-3-r2 m1) re-reads `hub` from the file, does NOT echo `hub_reason`)
- Modify: `src-tauri/src/mcp_server.rs:257` (one carrier sentence in the tool description)
- Modify: `tools/src/bin/ct.rs:580-604` (`ct wisdom deposit --repo-path`, absolutized against cwd, NO cwd default)
- Test: `src-tauri/tests/areas_carrier.rs`

**Interfaces:**
- Consumes: `resolve_deposit_hub` (Task 3).
- Produces: reply JSON gains `"hub": <id|null>` and `"hub_reason": "absent"|"wisdom_unconfigured"|"no_prefix"|"unresolved"` — the DOOR translates a missing `repo_path` param to `absent` BEFORE the resolver runs (the resolver takes `&str`; tier-3 r1 n1); deposit files may carry a leading `hub:` line (last in the block); both legs see it via the file (§6.8.4); hand-authored `hub:` honored only for configured-hub survivors (§6.8.4, gate-6 m4).

- [ ] **Step 1: Write failing tests** — (i) E2E: hub-only block and attribution+hub block render, re-parse, documents row carries the hub, chunk text free of frontmatter tokens (`chunker/mod.rs:168`), Librarian resolves the hub; (ii) Docker case: `repo_path` outside every checkout → `hub: null`, `hub_reason: no_prefix`, no `area` stamped; (iii) relative `repo_path` REFUSED at the door; (iv) both-absent render byte-identical to the pre-attribution pin; (v) reply contract for all four reasons; (vi) status re-reads `hub`, omits `hub_reason`; (vii) an `absent` hub emits NO production report line (§6.8.5's negative claim, asserted — tier-3 r2 n3); (viii) E2E of a HAND-WRITTEN `hub:` naming a live non-configured entity: deposit succeeds, reply carries `hub: null` / `unresolved` (the door-side answer beside Task 6's read-side test — tier-3 r3 n1).
- [ ] **Step 2: Run** `cargo --manifest-path src-tauri/Cargo.toml test --test areas_carrier --features test-utils` — expect FAIL.
- [ ] **Step 3: Implement** param + door + resolution + render + reply + status + tool description + CLI flag.
- [ ] **Step 4: Run** the `areas_carrier` integration target (`--test areas_carrier --features test-utils`) + full unit suite (`--lib`) — PASS.
- [ ] **Step 5: Commit** `feat(deposit): repo_path carrier — deposit-time hub resolution, hub: render, reply contract`

### Task 9: `ct wisdom areas rename` (§12) — **BLOCKED until Kurt rules on the `--path=` selector surface** (tier-3 r2 m3: the spec names none; the `--path=` proposal below is the plan's recommendation, not a settled surface)

**Files:**
- Modify: `tools/src/cmds.rs` + `tools/src/bin/ct.rs` (new `ct wisdom areas rename --path=<checkout path> <old> <new>` subcommand — the hub is DERIVED from `--path` via the SAME `resolve_scoped` rule as delivery, one resolver rule; tier-3 r2 MAJOR-2: the spec's §12.1 "against a hub whose areas file defines `new`" names no selector — this is the plan's proposed surface, FLAGGED for Kurt at plan approval. A `new` name not defined by that hub's areas file is REFUSED (tested).)
- Create: `src-tauri/src/db/areas_rename.rs` (the rename entry point — through `push_entries_outbox` `db/commit.rs:1342`, NOT a bare UPDATE; tier-3 r2 MAJOR-2 named the file)
- Test: CLI-level `tools/tests/ct_areas_rename.rs` (new) + lib-side unit tests inside `db/areas_rename.rs`'s `mod tests` (single placement each — tier-3 r2 m3)

**Interfaces:**
- Consumes: `cluster_ids` expansion (Task 3), `wiki_fact_outbox_payload` (Task 7), `reassert_entries` payload precedent (`db/restore_sync.rs:534-591`).
- Produces: one transaction: `UPDATE llm_wiki_entries SET area = :new, updated_at = :now WHERE area = :old AND entity_id IN :repo_group` (§12.2, §12.6); re-SELECT iterating the same set builds full-row payloads; one outbox Update per affected entry; actor stamped from `os_account()` (`db/proposals_review.rs:72` precedent) + one audit row per run in the vault-mirrored `stated/.confirmations.jsonl` house style (§12.5 — FILE destination, not a DB row; tier-3 r1 m4).

- [ ] **Step 1: Write failing tests** — the cross-id rename test: seed area rows on survivor AND loser ids; assert BOTH rows update and BOTH outbox Updates emit (gate-5 MAJOR-1); stale names on other groups untouched; `updated_at` bumped on every renamed row (§12.6); audit FILE row + actor stamp present; a `new` name not in the hub's areas file is REFUSED (tier-3 r2 MAJOR-2); `--yes` gate style matches `require_yes` (§12.1).
- [ ] **Step 2: Run** `cargo --manifest-path tools/Cargo.toml test --test ct_areas_rename` (NO `--features`) — expect FAIL.
- [ ] **Step 3: Implement** the command + transaction + payload + audit.
- [ ] **Step 4: Run** the rename tests (`tools`: `--test ct_areas_rename`; src-tauri: in-crate unit tests of `db/areas_rename.rs`) — PASS.
- [ ] **Step 5: Commit** `feat(cli): ct wisdom areas rename via outbox with merge-group expansion`

### Task 10: Measurement, pins, full verification (§11, §19)

**Files:**
- Modify: `src-tauri/tests/areas_write_path.rs` (attach-rate measurement §11.2 with the FOUR buckets: candidate-gate drops / model misses / no-owner-signal / hub-over-attach; §11.3 backlog count on a scratch brain)
- Measurement durable home: LEDGER + PR body ONLY — the Reviewed spec is NOT edited mid-implementation (tier-3 r2 m6; a spec amendment would be a post-merge errata lane)

- [ ] **Step 1: Run the attach-rate measurement** on the fixture; record the four bucket counts in the LEDGER (working copy, deleted in Task 11) AND the PR body (THE durable home — tier-3 r2 GLM m4).
- [ ] **Step 2: Run the §11.3 backlog count** on a scratch brain; disclose in the PR body.
- [ ] **Step 3: Full verification sweep** — both suites green (real output read); clippy base-vs-branch: no NEW findings; the four activity pins + message byte pin green; `git status --short` clean.
- [ ] **Step 4: Commit** `test(areas): attach-rate measurement + backlog count`

### Task 11: Independent review → same-PR cleanup → ready (SOP steps 8–11)

- [ ] **Step 1: Dispatch an INDEPENDENT reviewer subagent** over the full branch diff: diff-vs-spec/plan, re-run both suites, adjudicate every deviation. Fix findings with new commits.
- [ ] **Step 2: Final cleanup commit** — flip spec Status to `Implemented`, DELETE `specs/2026-11-14-areas/plan.md` + `LEDGER.md` (transient scaffolding), BEFORE the ready flip.
- [ ] **Step 3: `gh pr ready`** — triggers bot gauntlet; evaluate every bot finding (fold valid, skip with stated reason); CI green; Kurt merge-gates.

## Risks

- `WisdomItem` fixture fallout is the widest blast radius (PartialEq struct literals across tests) — budgeted in Task 4, mechanical.
- The band-predicate SQL fragment composes with `entry_eligibility_where` — keep composition ORDER stable (temporal first, band second) to protect the two-stage audit line (§9.2 accepted note).
- `globset` is a NEW dependency — named in the spec (§9.3); version pin at add time.
- If a test fixture can't construct a V29-only brain, Task 1 falls back to fresh-brain shape + migration-list assertion (stated in the task).

## Rejected implementation alternatives

- Enforcing the band predicate in `two_stage.rs` — spec forbids it (§9.2: `gated_entries` is the single enforcement point; `two_stage.rs` untouched).
- A `deposit_hub` cache table — gate-7 m1: `hub` has no join consumer; the file read suffices.
- Widening `wisdom_match_with_floor` for bindings — gate-7 m2: it would move the `tool_dispatch.rs:2229` pin call site and break the "pin-comment update only" claim.
- Defaulting `--repo-path` to cwd — §6.8.1: a wrong binding is permanent in an append-only archive.
