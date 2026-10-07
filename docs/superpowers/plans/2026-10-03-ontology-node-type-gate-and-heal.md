# Ontology Node-Type Gate + Heal + Duplicate Merge (wave 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement wave 1 of the ontology node-type gate spec — a write-time
node-type gate, an ontology-aware heal with signed-alias remap, a one-time
duplicate-entity merge sweep with local redirects, the OKF `entity_type` →
`doc_kind` internal rename, and the `ct ontology` CLI — per the converged
spec (`docs/superpowers/specs/2026-10-03-ontology-node-type-gate-and-heal-design.md`,
r19+r20 APPROVE WITH NITS consecutive).

**Architecture:** The spec is the single source of truth for every design
rule; this plan sequences the implementation into 10 tasks in dependency
order and assigns each spec requirement (R2.x.y) to exactly one task. The
central pattern: one shared entity-insert helper (`ImmediateTx` newtype)
through which all four production insert sites pass the gate; one shared
prefix-resolver core in config; one shared source-resolution core
(`Result`-based, three-variant) shared by gate and heal; heal = census →
remap → migrate, each phase its own transaction (heal NEVER merges —
merge is the standalone `ct wiki merge-duplicates` command, plan-p5-m4);
redirects are a
CT-owned table resolved read- and write-side, never bundle-format.

**Tech Stack:** Rust (`src-tauri` crate `curated-thoughts`, lib
`tauri_app_lib`), rusqlite (bundled), serde/serde_json, schemars 1
(feature `mcp-server`), clap (ct CLI in `tools/`).

**Spec:** `docs/superpowers/specs/2026-10-03-ontology-node-type-gate-and-heal-design.md`
— read it alongside this plan; every task below cites the R2.x.y
requirements it implements. OWNERSHIP RULE (r-p1-m3): where a requirement
spawns multiple tasks, the FIRST task owns the requirement and later
tasks own only their named slice. Requirement→task map (plan-p3-m2):
R2.2.1–R2.2.7→1; R2.2.8→1 (hash/config) + 3 (initial stamp) + 5
(report/FINAL-RULE) + 6 (merge precondition) + 8 (CLI gating); R2.3.0/
R2.3.1→2 (core) + 3 (apply, incl. edges); R2.3.2/2a→2 + 3 (gate side) +
5 (heal side); R2.3.3 gate side→3 + heal side→5; R2.3.4 gate side→3; R2.3.5→3
(writes the off-sourced directory row) + 5 (ledger fallback read);
R2.3.6→5 (plan-p11-MAJOR-2 ownership split); R2.4.2–R2.4.4→2
(helper/newtype/vocabulary — first task owns, plan-p4-m2);
R2.4.1/R2.4.5/R2.4.6 + the insert sites→3;
§2.1→3 (edge-disarm pins: `warn_strict_manifest_declares_no_edge_types`
+ `warn_ontology_unreadable`) + 2 (the 1a no-manifest row); §2.5→2
(swap + signature + `connection.rs:980` binding caller fix, plan-p10-m1)
+ 3 (gate call) + 9 (abort/observability rules at the `:997` `let _ =`
site);
R2.6.1–R2.6.4→5; R2.7.1–R2.7.6→6 (+7 read/write); R2.9.1→0; R2.9.2→0;
R2.9.3→5 (query) + 8 (display); §2.10→8; §2.8→4 (+8 MCP); §2.11→8;
§3 bookkeeping→10 (R2.2.8: Task 1 owns the watermark+
hash, Task 5 owns the report/FINAL-RULE consumption; R2.3.2: Task 2 owns
the resolver core, Tasks 3/5 own their consumers; §2.10: Task 8 owns the
sweep extension, Task 10 only docs/status). Inline `rN-*` tags in the spec are review
provenance, not normative.

## Global Constraints

- Scope: `src-tauri/src`, `tools/src` (ct CLI), `tools/tests/`,
  `src-tauri/tests`, `.github/workflows/ci.yml` (the new tools-test CI
  step, Task 8), plus spec/plan status flips. NO frontend changes, NO
  `core-llm-wiki`/TS engine changes, NO bundle-format changes (§5
  wave-2 boundary).
- The `rN-*` review tags in the spec are provenance. The normative text is
  the full sentence around them.
- Every DB write the gate/heal/merge performs is inside an IMMEDIATE
  transaction (R2.4.2's `ImmediateTx`; heal retypes per-row BEGIN
  IMMEDIATE per R2.6.1; merges one transaction per merge group). Heal
  NEVER merges (R2.6.1/R2.6.4 r12-M3): remap-before-merge is an ordering
  rule the STANDALONE merge command enforces via its precondition.
- No invented label ever enters `curated_entities.entity_type` (R2.4.4):
  skip-path `'concept'` literals are pre-existing behavior, not inventions;
  degrade outcomes land as UNTYPED in the origin ledger.
- The alias table is DATA (one place), not scattered match arms (R2.6.2).
- Never use `COLLATE NOCASE` or locale-aware comparison for survivor
  selection — byte-wise/BINARY in both Rust and SQL (R2.7.3).
- `heal --yes` requires `--confirm-drift <old-hash>` or
  `--waive-drift <old-hash>` whenever a drift report fires (R2.2.8 FINAL
  RULE); merge `--yes` never auto-applies (R2.7.6).
- CI gates (`.github/workflows/ci.yml`), from repo root (plan-p2-M5:
  ci.yml has NO fmt step and NO tools/ test step — fmt is a LOCAL
  convention; this PR ADDS the tools test step):
  - `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils -- -D warnings`
  - `cargo test --manifest-path src-tauri/Cargo.toml --features test-utils,mcp-server -- --test-threads=1`
  - `cargo clippy --manifest-path tools/Cargo.toml --all-targets -- -D warnings` (ci.yml:108-109)
  - NEW in this PR: `cargo test --manifest-path tools/Cargo.toml -- --test-threads=1`
    (so Task 5/8 CLI tests actually run in CI)
  - `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` (local pre-commit
    convention, not currently a CI gate)
  - The §2.8 schema tests are VACUOUS without `--features mcp-server`
    (R2.8): run them explicitly.
  - DECISION RULE: every DECISION (FINAL RULE gating, refusals, merge
    precondition) lives in `tauri_app_lib` functions tested under the
    src-tauri suite; `ct.rs` is thin parsing over them.
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
  Never squash-merge (repo rule).
- Task ordering is a hard dependency chain: schema DDL (0) → config
  core (1) → manifest vocabulary + NodeVocabulary + helper (2) → gate (3)
  → rename (4) → heal (5) → merge (6) → readers/writers (7) → CLI + MCP
  (8) → migration data + okf_migration alignment (9) → docs + status (10).
  Each task's tests must pass before the next begins.

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `src-tauri/src/config/mod.rs` | Modify | R2.2.1–R2.2.8: `folder_ontology`/`ontology_default` fields (`#[serde(default)]` map, `Option` scalar with `skip_serializing_if`), generic `resolve_prefix<T>` core (tie detection r19-m1), salvage of all three keys + `ingest_ontology_degraded`/dropped-key fields on `BrainConfig` (`#[serde(skip)]`), `raw_ingest`/`preserved_ingest` + `raw_ontology`, NotFound-only missing rule, writer-inventory fixes (`inference/mod.rs` rollbacks), delete `ingest_from_value_lenient` (+ its doc comment; test → tempdir fixture), struct-literal fixes (`:1248`, `librarian/mod.rs:582`), watermark hash (canonical, both-states) |
| `src-tauri/src/ontology_config.rs` | Modify | R2.2.5: rewrite stale `:36` doc comment; `skip_serializing_if` on `schema:` |
| `src-tauri/src/db/entities.rs` | Modify | R2.3.2/R2.3.2a: shared source-resolution core returning `Result<SourceResolution>` (`Resolved(paths)` / `HadEvidenceUnresolved` / `NoEvidence`, complete table, code's classification order); R2.7.5: redirect resolution in `get_entity`/`update_entity_summary`/`archive_entity` (survivor `id` in `EntityDetail`, archive-the-cluster rule, `&mut Connection` signatures); R2.4.2: mutators on `ImmediateTx` |
| `src-tauri/src/db/commit.rs` | Modify | §2.1/R2.3.0 (Task 3): edge-endpoint gating + skip-ladder application; R2.4.2 (Task 3): `create_entity_if_needed` through the shared helper; swap the rusqlite `Transaction` at `:2362` (already
IMMEDIATE-behavior) to the `ImmediateTx` newtype so the helper's
`&ImmediateTx` param type-checks (Task 2, plan-p7-m4) |
| `src-tauri/src/db/entity_gate.rs` | New | R2.3 ladder + R2.4.3 `NodeVocabulary` (`canonicalize`/`admit`/`from_manifest` incl. `fallback_node_type`) + R2.4.4 degrade ladder + R2.4.5 configuration-error rule + R2.4.6 origin ledger writes + R2.4.2 `ImmediateTx` newtype + shared insert helper (incl. upsert mode r6-M3) + `ensure_manifest_vocabulary` (idempotent; seed-set subset guard; object-shaped entries; r6-B1/r7-MAJOR-2/3) |
| `src-tauri/src/db/heal_ontology.rs` | New | Also owns `OntologyHealReport` (the CLI `CtHealOutput` payload, plan-p8-MAJOR-1). R2.6.1–R2.6.4: census (R2.9.3 rules), signed-alias table (DATA, target-declaredness check, `concept` disposition), retype mechanics (per-row IMMEDIATE, logged), queue routing; remap BEFORE merge is an ordering rule ENFORCED BY the
merge command's precondition (r12-M3: heal itself never merges);
`alias_remap_completed` marker (deleted by `clear_vault_tables`, r19-m2) |
| `src-tauri/src/db/merge_dedup.rs` | New | R2.7.1–R2.7.6: normalized-name grouping, deterministic BINARY survivor, re-entrancy, type-conflict queueing, empty-summary rule (r8-M3), redirects + path compression (2-hop/cycle tests), auto-merge gate, remap-precondition predicate (`llm_wiki_meta` marker, r17-m3) |
| `src-tauri/src/db/okf_migration.rs` | Modify | §2.5: abort semantics (GATE-without-fallback → abort without `okf_migrated_at`, r6-M2 observability at `connection.rs:997`), upsert via shared helper, `&mut Connection` |
| `src-tauri/src/db/connection.rs` | Modify | Caller fix for okf_migration signature + `:980`/`:997` `&mut conn` (Task 2 per plan-p10-m1 — plan-p14-m1 corrects the stale "Task 3"); best-effort ensure at DB open (r8-m6/r19-m5: name `AppDb` path — Task 2) |
| `src-tauri/src/db/bundle_apply.rs` | Modify | §2.5: `ensure_entity` through helper (SKIP-path `'concept'`+ledger) — its signature changes from `&Connection` to `&ImmediateTx` (`:712` today); swap the `:320` transaction (already IMMEDIATE-behavior) to `ImmediateTx` (plan-p7-m4); error propagation (no `.ok()` swallow, `:723-729`) |
| `src-tauri/src/db/queries.rs` | Modify | `clear_vault_tables`: add new tables to clear list (R2.9.1), delete `alias_remap_completed` marker (r19-m2) |
| `src-tauri/src/db/mod.rs` | Modify | Wire new modules |
| `src-tauri/src/librarian/synthesis.rs` | Modify (Task 3) | R2.4.1 prompt clause (`build_system_prompt`); redirect read site (Task 7) |
| `src-tauri/src/onboard/mod.rs` | Modify | `replace_ontology` caller (onboarding merge) |
| `src-tauri/src/librarian/mod.rs` | Modify | `IngestConfig` struct literal `:582` |
| `src-tauri/src/inference/mod.rs` | Modify | R2.2.4 writer inventory (r5-M1): `:240/:272` rollbacks → load → reset generation only → write |
| `src-tauri/src/entities_api.rs` | Modify | R2.4.2: `create_entity_cmd` opens IMMEDIATE tx (r1-MAJOR-3); mutator commands pass through redirect resolution |
| `src-tauri/src/okf/mod.rs` | Modify | §2.8 rename: field `doc_kind`/`DocKind` + `#[serde(rename = "entity_type")]` + doc comments; all test constructors |
| `src-tauri/src/okf/write.rs` | Modify | `write.rs:1351` rename site |
| `src-tauri/src/lib.rs` | Modify | §2.8 Tauri command param renames; `set_ontology_selection` as the only `raw_ontology` override writer (the onboarding merge is the other
sanctioned caller, per R2.2.5); schema-switch live-rule (r5-M2); `replace_ontology` caller inventory (Task 1: set_ontology_selection + onboarding merge) |
| `src-tauri/src/tool_dispatch.rs` + `src-tauri/src/mcp_server.rs` | Modify | §2.8 param renames; `$defs` DocKind; ontology writer NOT registered in MCP catalog (§6 item 7 test — NOT R2.9.2) |
| `src-tauri/src/wiki_graph.rs` | Modify (Task 2; Task 3 pins the read path) | r14-MAJOR-1: `WikiManifest` gains `fallback_node_type`; `parse_manifest` parses it (today it drops unknown keys); R2.3.0 read-path pin: unchanged tier_fact-only traversal (test pins it) |
| `tools/src/walk_list.rs` | Modify | r-p2-m4: `load()`→`write()` writer named by the spec — raw-ingest survival applies generically; add a degraded-write test |
| `tools/tests/` | Modify (mostly pre-existing) | EXTEND `ct_heal.rs` (drift/report-mode rows — NO separate `ct_heal_drift.rs`, plan-p1-M1/p4-m1) and `ct_trust.rs`; new `ct_ontology.rs`, `ct_merge.rs` only (CLI parse + report-mode tests; decision logic lives in src-tauri per Global Constraints) |
| `src-tauri/tests/mcp_write_integration.rs` | Modify | §2.8 rename imports/usages (r18-m3) |
| `src-tauri/tests/okf_migration.rs` + `src/db/okf_migration.rs` tests | Modify | `&mut conn` at 8 call sites (plan-p12-m1, Task 2 signature change) |
| `src-tauri/tests/wiki_graph.rs` | Modify | `WikiManifest` literal gains `fallback_node_type` (plan-p12-m2, Task 2) |
| `src-tauri/src/db/schema.rs` | Modify | `MIGRATION_V<next>` constants + version bump (Task 0, plan-p12-m6) |
| `tools/src/cmds.rs` | Modify | heal composition wiring (source-heal + ontology pass; FINAL RULE gate) |
| `tools/src/queries.rs` | Modify | `wiki_sweep_cmd` node-type extension plumbing |
| `tools/src/bin/ct.rs` + ct ontology module | New/Modify | §2.11: `ct ontology set --mode off|strict [--entity <id>] [--dir <prefix>] [--fallback <type>]` (+ off/strict entity semantics, reversal rule, degraded-config refusal, manifest-writer warnings), `ct heal` extension (drift report/`--confirm-drift`/`--waive-drift`/remap), one-time `ct wiki merge-duplicates` (report/`--yes` — spec's name;
`WikiCmd` subcommand group already exists, `ct.rs:167`), `ct wiki sweep` node-type extension (§2.10, census-driven) |
| `docs/superpowers/specs/2026-10-03-ontology-node-type-gate-and-heal-design.md` | Modify | Status → implemented |

Migration: ONE `MIGRATION_V<next>` in the existing chain — origin-ledger
table, `entity_redirects` table, `ct_entity_optouts` table — lands in
**Task 0** (Tasks 2–7 write to these tables; the DDL must exist first).
Task 9 keeps the okf_migration abort/observability alignment (the
incidental-off census MOVED to Task 5 — plan-p9-m4).

---

### Task 0: Schema DDL (migration tables first)

**Implements:** the three new tables (`entity_type_origin` ledger,
`entity_redirects`, `ct_entity_optouts(entity_id TEXT PRIMARY KEY, …)`)
in ONE `MIGRATION_V<next>` (plan-p6-m5: the spec at L1066 allows
      column-or-table for the origin record — the LEDGER TABLE option is
      chosen, named `entity_type_origin`; Tasks 3/5/9 use table
      semantics, never a column) — the origin table INCLUDES a
      source-directory column beside the original-label column
      (plan-p11-MAJOR-2: R2.3.5's off-sourced mint record needs the
      directory, else a stale-hash off entity climbs to rung 4 and gets
      retyped — the exact D8 case spec L846-847 pins), plus R2.9.1
      clear-list additions and the
R2.9.2 split-clear fix (`llm_wiki_entity_manifests` cleared on the clear
branch, vault-keyed on the restore branch — r14-MAJOR-1 prerequisite of
`--entity`; MECHANISM (plan-p5-m5: `llm_wiki_entity_manifests` is an
engine-owned table — the restore branch reinstalls the backup FILE via
`stage_backup` (`lib.rs:2039-2044`), it does NOT run
`clear_vault_tables`, so the "vault-keyed restore" behaviour comes from
the backup's own rows for the target vault; CT must NOT add columns to
the engine table — r10-M3 — and needs no restore-side code: the backup
already carries the right manifest rows. The CT-owned changes are the
CLEAR branch's non-tier DELETE and the three new tables only).

- [ ] Write `MIGRATION_V<next>` with the three tables; bump the schema
      version (`db/schema.rs` holds the `MIGRATION_V*` constants —
      plan-p12-m6, listed in File Structure; the schema-version pin test
      near `connection.rs:1077` is bumped with it).
- [ ] `clear_vault_tables` (queries.rs): add the three tables + DELETE the
      `alias_remap_completed` marker (r19-m2) + ENTITY-LEVEL manifest clear
      (plan-p2-M7: delete ONLY non-tier rows — KEEP `tier_fact`,
      `tier_wisdom`, `tier_working::%` — a bare DELETE would wipe the
      vocabulary and disarm the gate on every clear-branch switch);
      extend the `queries.rs:849-885` clear-list test (seed one tier row +
      one `ent_*` row; assert tier row SURVIVES, `ent_*` gone; account
      for any watermark row in the `:880` meta-count assertion — the
      drift watermark row SURVIVES `clear_vault_tables` (plan-p4-m5: it
      describes the host's config, not vault content; wiping it would
      re-arm first-run suppression. Test pins survival). The
      restore-branch case CANNOT live in the `queries.rs` test (the
      restore branch never calls `clear_vault_tables` — it reinstalls
      via `stage_backup` (`db/restore_sync.rs:262`), plan-p11-m6): the
      test lives in `restore_sync.rs` against `stage_backup` directly
      (plan-p12-m5: `switch_vault` is an async Tauri command with no
      test harness), named in Task 0's checklist.
- [ ] Tests: table existence post-migration; clear/restore behavior for
      all three tables + marker + manifest rows (§6 item 6 SLICE: the
      clear/restore rows only — plan-p4-m4; the watermark/migration rows
      are Task 5's and Task 9's).
- [ ] **r21 addendum — V26 origin-ledger revision (spec R2.4.6; land
      BEFORE Task 3, the ledger's first writer):** edit `MIGRATION_V26`
      in place — `original_type TEXT` (nullable: NULL = no label
      supplied), new `reason TEXT NOT NULL`; add the `OriginReason` enum
      (`degraded`/`unlabeled_landing`/`gate_skipped`/`alias_retype`/
      `queue_retype`) as the single writer-side owner (no SQL CHECK).
      V26 apply site: if `PRAGMA table_info(entity_type_origin)` lacks
      `reason`, DROP + re-CREATE when empty, fail loudly when non-empty.
      Update the `queries.rs` clear-list seed insert to the new shape.
      Tests: pre-r21 empty table rebuilt on open; pre-r21 non-empty
      table → open errors.

### Task 1: Config core — keys, generic resolver, salvage, degraded state, watermark hash

**Implements:** R2.2.1, R2.2.2 (incl. tie rule r19-m1 + `folder_tiers`
conservative-wins r20-m3), R2.2.3, R2.2.4 (NotFound-only, salvage-all-
three, `BrainConfig`-carried degraded state, `raw_ingest`+`preserved_ingest`,
`raw_ontology` + `replace_ontology` escape hatch r17-MAJOR-2, writer
inventory incl. `inference/mod.rs` rollbacks + writer-audit matrix cases),
R2.2.6, R2.2.8's watermark+hash half (plan-p5-m3: the watermark is
R2.2.8, NOT R2.2.7 — canonical serialization per r18-MAJOR-1,
computed in both states per r20-m2, heal-is-sole-writer; ownership
rule (stated once — plan-p6-m2 dedup): Task 1 = hash/config half,
Task 5 = report/FINAL-RULE consumption half, Task 8 = CLI gating
half — one requirement, three named slices), R2.2.5
(schema interplay live rule + no-fallback-stall research note), R2.2.5's
`skip_serializing_if` on `schema:` + `ontology_config.rs:36` comment
rewrite (r20-m4); R2.2.3's `skip_serializing_if` on `ontology_default`
(r19-m3); ALSO rewrite the stale `config/mod.rs:176-179` doc comment
("…or a load error all yield the default policy" — plan-p15-n1: false
after the load-failed route). (R2.2.7 is lifecycle-only — deliberately
no code.)

- [ ] DEFINE `OntologyMode` in `config/mod.rs` (plan-p11-m4: it does
      not exist today): `#[serde(rename_all = "lowercase")]` (case-
      sensitive — `"Off"` must FAIL, spec L426-429), derives
      `Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize`
      (full set mirrors `IngestTier` at `config/mod.rs:65` —
      `IngestConfig` derives Debug/Serialize/Deserialize at `:87`;
      PartialEq required by `PrefixOutcome<T: PartialEq>`; Copy/Eq for
      tie compare + hash).
      Test: `"Off"` rejected, `"off"` accepted.
- [ ] `IngestConfig`: add `folder_ontology: HashMap<String, OntologyMode>`
      (`#[serde(default)]`), `ontology_default: Option<OntologyMode>`
      (`#[serde(default)]` + `skip_serializing_if`); fix the two
      struct-literal sites the compiler flags.
- [ ] Extract generic `resolve_prefix<T>` (plan-p3-m5 signature base,
      UPGRADED per plan-p5-MAJOR-1 to a FOUR-STATE outcome — a
      two-state `Result<Option<T>, Tie>` cannot express R2.2.2's hold
      carve-out: "no prefix matched" and "absolute path couldn't be
      placed in the vault" must not both be `Ok(None)`, and the generic
      core cannot know whether the map holds `off` entries):
      `enum PrefixOutcome<T> { Match(T), NoMatch, Unplaceable, Tie(Vec<T>) }`
      (`T: PartialEq`). Core: normalize `\`→`/`, trim `/`,
      trim_start_matches("./") — plan-p5-m2, `tier_for` does this today
      at `config/mod.rs:107-108`, pin with a test. KEY/PATH ASYMMETRY
      (plan-p6-m4 + plan-p7-m1): trim `/` applies to CONFIGURED KEYS
      only (`:112`); the PATH gets `\`→`/` + `./`-trim; KEYS get NO
      `./`-trim (today's `:111-112` — a hand-edited `{"./ops": …}`
      key stays INERT for `folder_tiers`; pin that). Expose ONE
      `normalize_key` fn used by the resolver, `ontology_ties` AND the
      watermark hash so all three agree on which keys collide —
      UNMATCHABLE-KEY DIAGNOSTIC (plan-p9-m5: a hand-written
      `{"./ops":"off"}` key never matches any path — inert for
      `folder_tiers`, but for `folder_ontology` the silent result is
      `ops/` gated strict and `heal --yes` retypes it, the D8 harm):
      emit a load-time diagnostic for keys that cannot match (leading
      `./`, any segment that is empty, `.`, or `..` after splitting on
      `/` — SEGMENT-BASED per plan-p15-m1/m2: `..` as a WHOLE SEGMENT
      only (`Component::ParentDir` semantics per
      `trusted_links.rs:110-115`; a substring test would falsely flag
      real folders like `v1..v2`, which DO match today and keep
      matching) — plan-p10-m6: `tier_for` silently skips empty keys at
      `:113`, so
      `{"/":"off"}` or `{"":"off"}` meant as "whole vault off" is
      inert and the vault gets gated strict — same D8 harm); keep them
      inert for parity. Tests: `{"./ops":"off"}` AND `{"/":"off"}` →
      diagnostic + target NOT off; `{"ops//x":"off"}` inert + warned
      (plan-p15-m1). — longest-prefix by
      path-component depth, empty-check on the queried map; reports
      `Unplaceable` on no-effective-vault-root or
      `relativize_to_vault`→None — ABSOLUTE PATHS ONLY (plan-p11-m3:
      the core runs the `is_absolute()` check FIRST, mirroring
      `config/mod.rs:137-138` and spec L286-287 "relative paths … are
      NOT affected"; a relative path with no configured root resolves
      normally, never `Unplaceable`). Does NOT inspect mode values.
      Test row: relative path + no root + off map → normal
      resolution.
      ORDERING (plan-p8-m2, SCOPED plan-p10-m5): the
      Unplaceable-before-empty-check rule applies in the ONTOLOGY
      WRAPPER only — the wrapper knows its degraded/dropped state. The
      `tier_for_path` wrapper KEEPS its own `:133` empty-map
      short-circuit BEFORE calling the core (else every absolute path
      pays up to two `fs::canonicalize` calls on the per-document hot
      path, `walk_vault.rs:134/:138`, for a `Full` answer either way).
      Test rows: all entries dropped + vault moved → hold (ontology);
      empty `folder_tiers` + moved vault → `Full` via the
      short-circuit, no canonicalize (tier).
      WRAPPER TESTS (plan-p14-MAJOR-1): dropped child under VALID
      parent (`{"ops":"strict","ops/x":"Off"}` dropped, path `ops/x/`)
      → `Hold` — the spec's "`Off` typo under prefix `x`" matrix case
      has no valid parent and would pass with this bug present; valid
      child under dropped parent → the child's mode; dropped-default
      (`{"ingest":{"ontology_default":"Off"}}` dropped, no schema
      config) → `Hold` (plan-p15-m4, owned by Task 1 — the unreadable-
      schema row stays Task 3's per the map). Task 1's ontology wrapper returns a CONFIG-LEVEL
      outcome `enum OntologyLookup { Mode(OntologyMode), Hold, Climb }`
      (plan-p13-m1: `HadEvidenceUnresolved` lives in Task 2's
      `SourceResolution` — not available yet; Task 3/5 map `Hold` →
      `HadEvidenceUnresolved`). FULL MAPPING (plan-p14-MAJOR-1 — the
      resolver never sees dropped entries, salvage removed them, so the
      wrapper checks the dropped list ITSELF before anything else):
      (0) global degraded (load-failed / non-object parts) → `Hold`;
      (1) normalized path at or under ANY dropped `folder_ontology`
      prefix → `Hold`, UNLESS a valid entry deeper than the dropped one
      also matches (valid child under dropped parent → the child's
      mode); (2a) `NoMatch` + dropped `ontology_default` value → `Hold`
      (plan-p15-m4, spec L506-507 — else
      `{"ingest":{"ontology_default":"Off"}}` climbs to schema strict
      and `heal --yes` retypes an opted-out brain); (2b) `NoMatch` +
      `ontology_default` absent + `ontology.schema` unreadable →
      `Hold` (spec L547-549); (2) resolver `Tie` → `Hold`; (3) resolver `Unplaceable` +
      (≥1 `off` OR degraded/dropped entry, R2.2.2 scope) → `Hold` —
      NEVER a climb (D8: vault `tier_for_path` maps `Unplaceable` →
      `Full` — wording "preserves today's shipped `Full`" (plan-p8-m5 +
      plan-p10-m7: never call it "conservative" — in the tier ranking
      `none` < `chunks-only` < `full` makes `Full` the LEAST
      conservative tier, and the word invites someone to "fix" it to
      `None`; `config/mod.rs:140-144` behaviour). Tie rule direction
      stated explicitly: the MINIMUM by `none` < `chunks-only` <
      `full` wins.
      `Tie(Vec<T>)` carries the values so the `folder_tiers` caller can
      pick the most-conservative tier by EXPLICIT ranking `none` <
      `chunks-only` < `full` — not enum declaration order. TIE
      DIAGNOSTICS fire ONCE at load (in `load_lenient`/`ontology_ties`,
      plan-p11-m5 — NOT in the resolver: `tier_for_path` runs per
      document on the walk hot path, so an in-resolver diagnostic would
      spam stderr per document); the resolver itself is silent.
      `tier_for_path` and the ontology lookup both sit on the core.
- [ ] Degraded state as `#[serde(skip)]` `BrainConfig` fields set in
      `load_lenient` (NOT LoadReport — r4-M1/r5-m1/r5-M5): flag + dropped
      `folder_ontology` prefixes + `ontology_default`-dropped bool;
      `ingest_policy_for_db` exposes them and the `IngestPolicy` carrier
      fields (r6-M4); MAP-WIDE tie scan (plan-p5-m1): a strict parse
      never passes through salvage, so ties need a standalone
      `fn ontology_ties(&IngestConfig) -> Vec<(String, OntologyMode)>`
      helper (same-normalized-key conflicting values across the whole
      map) — consumed by the watermark hash's degraded encoding
      ({degraded:true, conflicting keys EXCLUDED}, spec L621-627) and
      the refuse-while-degraded checks; a strictly-parsed config with
      internal ties sets the same degraded-tie state. ACCEPTED LIMIT
      (plan-p15-n2): REPEATED literal keys (`{"ops":"off","ops":"strict"}`)
      are invisible — serde keeps the last value silently; not
      detected. Noted, accepted. TIER TIE SCAN
      (plan-p14-m2): a matching `folder_tiers` tie scan at load emits
      the spec L306-309 loud diagnostic + conservative-wins outcome —
      BOTH load paths run the scans (`load()`'s strict success arm
      `:433-531` never calls `load_lenient`, so each arm calls the
      scans itself). RESOLVER CONTRACT: same-VALUE keys resolve to
      `Match` (harmless), never `Tie` (spec L306) — pinned by test.
- [ ] LOAD-FAILED ROUTE (plan-p6-MAJOR-1: today `ingest_policy_for_db`
      turns ANY `load_lenient` `Err` into `IngestPolicy::default()` at
      `config/mod.rs:209-212` — malformed top-level JSON (`:591`) and
      non-object root (`:592-597`) never reach salvage, so a truncated
      config.json yields a NON-degraded policy and every path climbs to
      strict rungs; a truncated `{"ops":"off"}` would let `heal --yes`
      retype `ops/`, breaking D8): when (a) the `:191` read errors with
      anything other than NotFound, (b) UTF-8 decode fails, or (c)
      `load_lenient` returns `Err` at all — produce a policy with
      `ingest_ontology_degraded = true`, NEVER `default()`. CARRIER
      (plan-p9-m8: on `Err` there is no `BrainConfig` to hold the
      flags): `IngestPolicy` carries `ingest_ontology_degraded`
      independently of `BrainConfig`, and consumers that read
      `BrainConfig` (e.g. `ct ontology set`) refuse whenever
      `load_lenient` returns `Err`. The
      "only-NotFound" wording elsewhere covers these too. Test: truncated
      config.json → degraded policy (Task 1); heal half (corrupt config +
      `heal --yes` → zero retypes) is Task 5's, §6 item 2a.
- [ ] Salvage: per-key independent (r15-MAJOR-1), scoped-hold data;
      GLOBAL degraded triggers (complete list, plan-p11-m2): any
      `load_lenient` `Err` — malformed JSON (`:591`), non-object root
      (`:592-597`), UTF-8 failure, `VaultPathNotString` (`:714`) — plus
      non-NotFound I/O on the `:191` read, a non-object `ingest`, AND a
      non-object `folder_ontology` value (spec L507: holds everything —
      `{"ingest":{"folder_ontology":"off"}}` must NOT resolve as an
      empty non-degraded map); unparseable
      `ontology` → `ontology_unparseable` (existing flag) consumed by rung 3
      (r16-MAJOR-1 hold rule; the hold diagnostic tells the user to fix
      the config and retry — the write-protection rules guarantee the
      fix is possible). LOAD-FAILED policy TIERS (plan-p13-m6): `tiers`
      stays `IngestConfig::default()` (every path `Full` — today's
      shipped behaviour); ONLY the ontology carrier fields change. Never
      salvage partial `folder_tiers` from an unreadable file. CACHING
      (plan-p15-m3): the degraded policy IS cached keyed on the file
      bytes (parse/UTF-8 failures have bytes to key on) so the hot path
      does not re-parse + re-log per document; for non-NotFound READ
      errors (no bytes) the diagnostic prints once per process per
      path. Pinned in the truncated-config test.
- [ ] Write protection: `raw_ingest` (skip-field, leave-on-disk-untouched
      rule r15-m1/r17-m1), `preserved_ingest` (unknown keys, both load
      arms r13-m2), `raw_ontology` + `BrainConfig::replace_ontology` (only
      `set_ontology_selection` + onboarding merge call it); `ct ontology set` refusal while
      degraded (plan-p2-M4: `ct trust` is NOT an ingest-mutating writer —
      it stays WORKING while degraded and its test asserts the raw ingest
      block survives byte-for-byte in the spec's matrix-case wording,
      spec L383-386 — implemented as VALUE-identical on `root["ingest"]`
      per plan-p4-m6).
- [ ] Writer inventory: fix `inference/mod.rs:240/:272` rollbacks; run
      the writer audit with the regexes `BrainConfig::load(_lenient)?\(`,
      `BrainConfig::default\(\)` AND `\.write\(&paths\)` (plan-p12-m4:
      the load-only regex misses the exact bug class — the
      `BrainConfig::default().write(&paths)` rollbacks at
      `inference/mod.rs:240/:272`; also surfaces `onboard --force`'s
      `fs::write(config_path, "{}")` at `onboard/mod.rs:186`, listed as
      a DELIBERATE backed-up exception in the matrix); matrix cases
      (lib.rs setters, privacy toggle, onboard merge, vault config,
      provider-fail rollback keeps opt-outs, onboard --force blank).
- [ ] Same-bytes parse rule (r11-m5): new `load_lenient_from_str(&str)`
      (plan-p11-m7: named so both call sites route through it);
      `ingest_policy_for_db` parses from the SAME bytes that key the
      cache — `:191` vs `:204` double read eliminated — AND `load()`'s
      fallback arm (`:544`) routes through it too (today it re-reads
      the file, so `raw_ingest` is captured from a different read than
      the strict parse used) + cache-invalidation test;
      non-UTF-8 config bytes = DEGRADED not missing (r12-m5); path-less DB
      is NOT degraded (r14-m6).
- [ ] Cleanup (plan-p3-m1): DELETE `ingest_from_value_lenient` +
      its doc comment (`config/mod.rs:343-366`) and repoint the `:1240`
      test at a tempdir `BrainPaths` fixture (R2.2.4 r20-m5); rewrite the
      `ontology_config.rs:36` comment + `skip_serializing_if` on `schema:`
      (R2.2.5 r20-m4); extract `preserved_ingest` in BOTH arms —
      `load()` success arm (pattern at `:493-521`) AND `load_lenient`
      (`:695-708`).
- [ ] Watermark hash helper: canonical input (normalized keys via the
      ONE `normalize_key`, BTreeMap, fixed field order, both-states
      degraded encoding, map-wide tie scan) PLUS the rung-3 conditional
      inputs (plan-p12-m3, spec r8-M4): when `ontology_default` is
      ABSENT, `ontology.schema` + `ontology_unparseable` are hash
      inputs — else an Off→strict schema switch fires no drift report.
      UNMATCHABLE KEYS (plan-p14-m5): inert keys (`"./ops"`, `"/"`,
      empty-after-normalization) COUNT toward the hash — fixing
      `./ops` → `ops` is a semantic config change and SHOULD fire a
      drift report. Pinned by test. Tests: insertion-order identity;
      schema change + absent
      `ontology_default` → hash CHANGES; same change + present
      `ontology_default` → hash UNCHANGED.
- [ ] Tests (CONFIG-LEVEL ONLY — plan-p2-M1: CLI/heal/gate-dependent
      cases live with their owning tasks): full+off same-prefix,
      corrupted-salvage, scoped hold, raw-ingest survival (plan-p4-m6: `write()` re-serializes the whole
      file pretty-printed, so assert the `ingest` key VALUE-identical —
      parsed `serde_json::Value` equality on `root["ingest"]` — not
      literal file bytes) — implemented as a src-tauri
      `load()`→mutate `trusted_links`→`write()` test (the `ct trust`
      CLI surface itself is Task 8's, per plan-p3-m6), EACCES, tie rule,
      tier tie conservative-wins, insertion-order hash. The `ct ontology set`
      refusal test is Task 8; the first-run watermark (heal) test is
      Task 5; the unparseable-schema hold test is Task 3.

### Task 2: Manifest vocabulary + NodeVocabulary + ImmediateTx helper

**Implements:** R2.4.2 (helper + newtype + upsert mode + caller list),
R2.4.3, R2.4.4 (incl. `fallback_node_type` key, reader, ensure step with
seed-set subset guard, object-shaped added entries, declare-or-report),
R2.3.0/R2.3.1/R2.3.0-skip wiring for `NodeVocabulary::from_manifest`,
R2.3.2/R2.3.2a (the SHARED source-resolution core — plan-p11-MAJOR-1).

- [ ] FIX the 8 `run_okf_migration` TEST call sites in this task
      (plan-p13-m2: `cargo test` compiles every target, so Task 2's
      check gate needs them): `src/db/okf_migration.rs:240/:245` +
      `tests/okf_migration.rs:58/:90/:122/:152/:182/:183` — `let mut
      conn` / `&mut conn` (`:183` uses `&conn` inline).
- [ ] `ImmediateTx<'c>` newtype + `ImmediateTx::begin(&mut Connection)`;
      no behavior-getter reliance (r3-m7); `Deref<Target =
      rusqlite::Transaction<'c>>` (plan-p12-MAJOR-1 — NOT Connection:
      `commit.rs:2434/:2504` call `tx.rollback()` and `:2539` passes
      `&tx` to `finalize_proposal_status_guarded(&rusqlite::Transaction)`
      at `:2104` — a Connection-deref cannot compile there; Transaction
      derefs transitively to Connection so `&tx → &Connection` helpers
      still work) + CONSUMING `commit()` AND `rollback()` passthroughs
      (`commit.rs:2362` uses execute/commit/rollback + the guarded
      helper; `bundle_apply.rs:320` uses only `&tx → &Connection`
      helpers + `tx.commit()` at `:707`); perform the SWAPS named in the File
      Structure table (plan-p8-m3): `commit.rs:2362` and
      `bundle_apply.rs:320` (both already IMMEDIATE-behavior — swap the
      rusqlite `Transaction` for the newtype) and
      `okf_migration.rs:173`'s raw `BEGIN IMMEDIATE;` →
      `ImmediateTx::begin` — this INCLUDES the
      `run_okf_migration(&Connection)`→`(&mut Connection)` signature
      change and its caller fix `let mut conn` at `connection.rs:980`
      (plan-p10-m1: Task 2 must compile, so it owns BOTH; no other
      task re-owns them). Test: existing callers pass unchanged.
- [ ] Single-hop redirect resolver WITH cycle guard (plan-p2-M1: the
      helper's redirect check needs it now; Task 7 only APPLIES it at
      reader/writer sites).
- [ ] Shared insert helper: gate check + redirect check + existence check
      + insert, atomic; error PROPAGATION (no `.ok()`); explicit upsert
      mode (name/summary/updated_at only — r6-M3); caller-supplied-id
      support (r9-m1).
- [ ] `ensure_manifest_vocabulary`: idempotent; full vocabulary work
      (add `document`+`process` as OBJECT entries + write
      `fallback_node_type`) under the seed-set SUBSET guard; foreign
      manifests → fallback-only declare-or-report; memoized per
      entity+manifest-content (r11-m4: the memo is recorded ONLY after
      commit); best-effort at `AppDb` open, ORDER PINNED migrate →
      ensure → okf_migration (plan-p10-m4: ensure AFTER okf_migration
      would let an upgraded brain with a pending conversion +
      pre-wave-1 manifest hit Task 3's gate-without-fallback abort at
      open). Test pins the order.
- [ ] SHARED SOURCE-RESOLUTION CORE (plan-p11-MAJOR-1; spec L756-764,
      L795-815 — "SAME resolver core … no second implementation"):
      `Result<SourceResolution>` in `entities.rs` with THREE outcomes
      (`Resolved` / `HadEvidenceUnresolved` / `NoEvidence`, R2.3.2a
      classification table rows); error PROPAGATION at both swallow
      sites (`entities.rs:213-217`, `:251` — a DB fault must NOT
      masquerade as "no source" and climb to strict, spec L792);
      librarian-token shape checked FIRST; `wiki_context` remains a
      second DISPLAY wrapper that degrades on error. Task 3's hold
      decision and Task 5's heal census both consume THIS core.
- [ ] `wiki_graph.rs`: extend `WikiManifest` + `parse_manifest` for
      `fallback_node_type` (r14-MAJOR-1 — without this every strict brain
      reads as no-fallback and every mint is held); field is
      `#[serde(default, skip_serializing_if = "Option::is_none")]
      Option<String>` (plan-p12-m2: `WikiManifest` serializes inside
      `WikiOntologyResult` → frontend/MCP — additive-only output keeps
      the NO-frontend-changes constraint; `tests/wiki_graph.rs:84`
      struct literal is touched — listed in File Structure).
- [ ] `NodeVocabulary::from_manifest` (parses `fallback_node_type`;
      empty-declared-set rule; ensure → `wiki_get_ontology` →
      `NodeVocabulary` end-to-end test) + `canonicalize` + `admit` (alias-target
      declaredness → queue, r2-M1).
- [ ] Tests: §6 items 1a, 2 and the R2.4.4 matrix cases (pre-wave-1
      manifest, fresh-install-after-migration, no-preferred-fallback loud
      error, hand-stripped key re-ensured, foreign manifest untouched,
      engine-rewrite survival of the CT table).

### Task 3: Write-time node-type gate

**Implements:** R2.4.1 (prompt clause), R2.3 ladder application at all
four insert sites, R2.4.5, R2.4.6, R2.3.4 (mint-path split r4-m6), gate
side of R2.3.2/R2.3.2a (hold classification), R2.3.0 edge-endpoint
strict-wins + the §2.1 entity-level opt-out cascade short-circuit for
EDGES (commit.rs:364-369), §6 item 1b's per-direction matrix rows.

- [ ] Prompt closed-set clause appended in `librarian/synthesis.rs`
      (`build_system_prompt`, `:625` — synthesis pattern per edge clause;
      NOT commit.rs); resolved from `tier_fact` for new entities;
      `format_candidates_section` (`synthesis.rs:541`) joins the Task 7
      redirect read list.
- [ ] Gate at the four sites through the helper: LLM synthesis
      (`create_entity_if_needed`), GUI (`entities_api` opens IMMEDIATE),
      bundle (`ensure_entity`: SKIP-path `'concept'`+ledger UNTYPED row,
      abort-on-config-fail, fallback landing + queue), okf_migration
      (the gate call lands here; it compiles against Task 2's
      `ImmediateTx` swap, signature change, and `connection.rs:980`
      binding caller fix — plan-p10-m1/plan-p11-m1; Task 9 keeps
      only the loud logging/diagnostic at the `:997` `let _ =` site;
      (the 8 TEST call-site fixes belong to TASK 2 — plan-p13-m2 moved
      them there, since `cargo test` compiles all targets and Task 2's
      gate must compile; Task 3 only adds the gate call).
      The LADDER-OUTCOME DECISION
      (abort-without-`okf_migrated_at` on gate-without-fallback vs the
      §2.5 SKIP path `'concept'`+ledger row) is THIS task's — plan-p3-m4:
      between Tasks 3 and 9 the aborts would otherwise disappear behind
      `let _ =` at `connection.rs:997`.
- [ ] Degrade ladder implementation + origin-ledger UNTYPED records;
      off-sourced mints write the R2.3.5 directory row (source
      directory path + original label — plan-p11-MAJOR-2 write side); every
      write is `INSERT OR IGNORE` with its `OriginReason` per the
      R2.4.6 table (first origin wins).
- [ ] Edge gating (R2.3.0): gate an edge when EITHER endpoint resolves
      strict; entity-level opt-out on an endpoint short-circuits the edge
      cascade (§2.1, commit.rs:364-369); per-direction matrix tests
      (§6 item 1b).
- [ ] Initial watermark stamp at first gate/heal resolution (r13-MAJOR-3:
      `INSERT OR IGNORE`, skipped on read-only connections — refines
      Task 1's heal-is-sole-writer rule).
- [ ] Tests: §6 items 1b (edge per-direction rows — the bullet above),
      2 (all four paths; plain `&Connection` does
      not compile; upsert preserves `entity_type`; SG6 held proposals
      keep facts; degrade ladder; no invented label).

### Task 4: OKF rename (§2.8)

- [ ] Field/enum → `doc_kind`/`DocKind` + `#[serde(rename = "entity_type")]`
      + doc comments; fix ALL compile sites incl.
      `tests/mcp_write_integration.rs` (r18-m3) and `write.rs:1351`.
- [ ] `doc_kind:` key FAILS to parse (no alias, r11-m3); `$defs` DocKind
      test under `--features mcp-server`; description assertions.
- [ ] §6 item 5 rows: serialize round-trip (`entity_type` key survives
      `serde_json::to_value`) and `KNOWN_KEYS`-unchanged pin (r14-m3).
- [ ] Check command: `cargo test --manifest-path src-tauri/Cargo.toml --all-targets --features test-utils,mcp-server -- --test-threads=1` (r-p1-m2: integration tests import test-utils-gated items; r-p2-m3: full path since commands run from repo root; `--test-threads=1` matches CI — plan-p9-m6: the per-config-path cache at `config/mod.rs:182` makes parallel runs flaky).

### Task 5: Ontology heal

**Implements:** R2.6.1–R2.6.4, R2.3.2/R2.3.2a heal side (report-only
scope + complete table + conservative rollup), R2.3.3 heal side
(strict-wins across ALL hash matches), R2.3.5 heal side (ledger
fallback when live resolution comes back empty — plan-p11-MAJOR-2 read
side), R2.3.6 (report/queue), R2.9.3 in
full (census QUERY lives here per plan-p3-M2 — Task 8 owns only the
`ct`-facing REPORT display, Task 9 nothing), drift report + FINAL RULE
(R2.2.8), `alias_remap_completed` marker.

- [ ] STDOUT CONTRACT (plan-p7-MAJOR-1: `heal_run`'s doc comment
      `cmds.rs:244-246` + the two `ct_heal.rs` assertions at `:148-150`
      and `:221-223` pin stdout to EXACTLY ONE JSON object — a stray
      `println!` for the drift report or retype log breaks them and any
      `ct heal --yes | jq` script): stdout stays one JSON object. SHAPE
      (plan-p8-MAJOR-1: `HealSummary` at `db/heal.rs:25-33` is `Copy`
      with three `usize` fields — a `Vec` field cannot compile, and the
      GUI/scheduler path returns `HealSummary` directly — the struct
      stays UNTOUCHED): the CLI serializes a NEW output struct in
      `tools/src/cmds.rs`,
      `CtHealOutput { #[serde(flatten)] summary: HealSummary, ontology:
      OntologyHealReport }`, `OntologyHealReport` living in
      `heal_ontology.rs` with `drift {old_hash, new_hash, confirmed},
      retyped, queued, skipped_reason` (spec R2.6.3: retypes "logged in
      the heal summary" — the flattened CLI output IS that summary);
      flattening keeps `evaluated`/`soft_deleted`/`edges_purged`
      top-level so `ct_heal.rs:149-156`/`:222-225` keep passing. Update
      the `ct_heal.rs:7` module-doc shape note. Human-readable
      drift/census text goes to STDERR (as the refusal path already
      does at `ct.rs:600`). FAULT HANDLING (plan-p8-m1: source-heal
      commits per-row IMMEDIATE at `heal.rs:80` BEFORE the ontology
      pass — a census DB fault that must propagate per R2.3.2a would
      otherwise leave mutations done but stdout empty): the ontology
      pass catches its own errors into `ontology.error` /
      `ontology.skipped_reason`; the single JSON object is ALWAYS
      printed; `heal_run` then returns non-zero.
      Extend `ct_heal.rs`: the drift-blocked `--yes` row still parses
      stdout as ONE object, `soft_deleted > 0`,
      `ontology.skipped_reason = "unconfirmed_drift"`. NO-CONFIG
      REGRESSION PIN (plan-p13-m5): the existing `ct_heal.rs` fixtures
      (`seed_heal_fixture`, `init_brain_db`) have no `config.json` and
      only migration-seeded manifests — pin that on such a brain the
      ensure + census is a NO-OP (`ontology` section present, no
      `error`, exit 0) so `heal_with_yes_heals_purges_and_prints_summary_json`
      and `heal_with_yes_on_clean_brain_exits_zero_with_zero_summary`
      stay UNMODIFIED as the exit-0 regression pins (a "no preferred
      fallback" loud error must not fire on a fresh fixture).
- [ ] ENSURE BEFORE CENSUS (plan-p7-m2: `heal_run` opens via
      `open_rw` + `migrate_open_db` (`cmds.rs:252-264`), NOT
      `AppDb::open_with_config`, so the open-time ensure never fires
      for `ct heal` — a pre-wave-1 manifest would misclassify every
      `document` row as drift): the `--yes` path calls
      `ensure_manifest_vocabulary` on its connection AFTER
      `migrate_open_db` and BEFORE the census. Test: pre-wave-1
      manifest + `ct heal --yes` → zero `document` rows queued. The
      same ensure-before-consult rule applies to the
      `ct wiki merge-duplicates` and `ct wiki sweep` entry points
      (Task 8). MIGRATION TOO (plan-p9-M3: `ct wiki sweep --yes` opens
      via `open_rw` with NO `migrate_open_db` at
      `tools/src/queries.rs:684-686`, report mode via `open_ro` at
      `:673-674` — first run after upgrade would fail with
      `no such table: entity_redirects`): EVERY new `--yes` entry point
      calls `migrate_open_db` then `ensure_manifest_vocabulary`; every
      READ-ONLY report path checks table existence and reports
      "schema pending (read-only)" instead of failing (the `"?"`
      fallback pattern, `ct.rs:584-599`) — `ct heal` without `--yes`
      included (its read-only census cannot migrate). Test per entry
      point against an old-schema database.
- [ ] Retype provenance (spec R2.4.6/R2.3.6, r21): every heal retype
      writes its origin row in the SAME per-row IMMEDIATE transaction —
      `alias_retype` for signed-alias remaps, `queue_retype` for approved
      queue items — `INSERT OR IGNORE` with the pre-retype label (first
      origin wins). Heal surfacing filters by `reason` per the R2.4.6
      table (`alias_retype`/`queue_retype` never surface as drift). Test:
      `heal --yes` remap of a ledger-less `agent` row → `alias_retype`
      row with `original_type = 'agent'`; re-run → no new surfacing.
- [ ] Census → drift report (echo old+new hash → STDERR per the stdout
      contract above) → remap (signed table,
      target-declaredness → queue; `concept` disposition per r8-M2 +
      r16-m1 gate/heal parity) → migrate → queue remainder.
- [ ] WAIVE SEMANTICS (plan-p9-M2, spec L654-658): `--waive-drift`
      acknowledges the mismatch and PROCEEDS WITHOUT retypes, remaps,
      OR watermark storage — the waive event is recorded in the heal
      summary OUTPUT only (add `waived: bool` + `old_stamped_at` to
      `OntologyHealReport.drift` — spec L646-647 requires old hash +
      timestamp). Waive does NOT unlock the ontology pass; it ends the
      drift REPORTING state for this run. EXIT CODES (plan-p13-m4):
      waive with MATCHING hash → exit 0, `drift.waived=true`,
      `skipped_reason="drift_waived"`; MISMATCHED hash on either flag →
      exit 1 + loud stderr (echoed pair not confirmed); unconfirmed
      drift → exit 1, `skipped_reason="unconfirmed_drift"`. NO-DRIFT
      CASE (plan-p14-m3): `--confirm-drift`/`--waive-drift` given when
      NO drift report fires (hashes match, or no watermark row yet) →
      the flag is IGNORED with a stderr note, exit 0 (scripts that
      always pass it keep working). Test row pins it. Test rows
      pin all three outcomes. Test: `heal --yes
      --waive-drift <h>` → zero rows retyped, watermark unchanged.
      Task 6 note: for MERGES the FINAL RULE lists merges as gated, so
      `--waive-drift <h>` (unlike heal) DOES unlock merge — one run.
      NEITHER flag writes the watermark from the merge path (spec
      L635-636: heal is the SOLE watermark writer — plan-p10-m2b;
      confirmed merge included). Tests in Task 6: waived merge
      proceeds AND confirmed merge proceeds, watermark row unchanged
      after both. FIXTURE (plan-p10-m3: waive skips the remap pass, so
      the `alias_remap_completed` marker — set only at the END of a
      successful `heal --yes` remap, spec L1205-1207 — is never set; a
      fresh-drift waived merge would be blocked by the merge
      precondition): the waived-merge test SEEDS the marker; plus a
      row pinning "waive with no marker → precondition refusal".
- [ ] `--yes` gating: drift report + confirm/waive hash (CLI form per
      r6-m5/r4-m1); remap per-row IMMEDIATE; report-only rules (unresolved
      sources, off-sourced, stale-hash + empty-map scope r18-m2).
      OWNS the clap flags (plan-p5-MAJOR-2: Task 5's tools tests cannot
      express `--confirm-drift <hash>`/`--waive-drift <hash>` or observe
      exit 1 if they arrive in Task 8 — today `Cmd::Heal` has only `yes`,
      `ct.rs:97-101`; the refusal arm already returns `Ok(1)` at
      `:604`, but the `--yes` arm always exits 0 at `:606-607`
      (plan-p6-m1 correction): extend the variant to
      `Heal { yes, confirm_drift: Option<String>, waive_drift:
      Option<String> }` — KEEP the existing `yes` field (plan-p9-m7) —
      in `ct.rs` IN THIS TASK, and change `cli_common::heal_run` to
      return an exit code (the refusal arm's exit-1 path is testable
      here). CLAP ATTRS (plan-p10-M1: `conflicts_with = "yes"` would
      FORBID `--yes --confirm-drift <h>` — the only form the FINAL
      RULE accepts — banning drift clearance entirely): both flags use
      `#[arg(long, requires = "yes")]` (refuse WITHOUT `--yes`), and
      are mutually exclusive with each other (`conflicts_with` between
      the two, plan-p10-m2a). CLAP TEST (plan-p14-m4): `ct heal
      --confirm-drift h` WITHOUT `--yes` is a usage error (exit ≠ 0)
      and touches nothing — pinned in `ct_heal.rs`, since clap's
      `requires` vs default-false behaviour is version-dependent.
      Task 8's heal slice is report DISPLAY
      only.
- [ ] NON-`--yes` arm contract (plan-p3-M1: today `Cmd::Heal{yes:false}`
      is a hard refusal — `ct.rs:570-605` read-only open, print count,
      exit 1; `tools/tests/ct_heal.rs:81-133` pins no-mutation and
      no-fresh-DB-creation): the arm KEEPS the read-only open; it ADDS a
      read-only ontology census + drift section; the manifest ENSURE is
      computed IN MEMORY and reported as "ensure pending (read-only)"
      per R2.4.4 r12-m4 — NO watermark stamp, NO brain.db creation; still
      exits 1; the census + drift section go to STDERR (plan-p10-m8:
      keeps the refusal arm's empty stdout so scripts reading stdout
      keep working). `--yes` with unconfirmed drift: SOURCE-HEAL RUNS (rows
      ARE changed — `heal.rs:80` per-row IMMEDIATE), the ontology
      section is SKIPPED with `skipped_reason = "unconfirmed_drift"`,
      report printed, exit 1 (plan-p9-M1: NO "no mutation" wording —
      that reading gates all of `heal_run` and contradicts the
      composition row below). EXTEND `tools/tests/ct_heal.rs` — do NOT
      add a separate `ct_heal_drift.rs`.
- [ ] Composition with the EXISTING `ct heal` (r14-M3/plan-p1-M3): the
      ontology pass COMPOSES with `db::heal::heal_invalid_sources_conn`
      (source-heal runs first, then the ontology census/remap). FINAL
      RULE scoping (plan-p4-MAJOR-1: the spec's FINAL RULE lists only
      "retypes, alias remaps, merges" — source-heal is an existing
      unrelated pass and MUST NOT be blocked by an ontology drift
      report): the drift gate blocks ONLY the ontology section — a
      script's `ct heal --yes` with unconfirmed drift still soft-deletes
      ungrounded rows, then prints the drift report and skips the
      ontology remap (exit 1). Test pins source-heal mutated +
      ontology-remap skipped. The DEGRADED-CONFIG refusal is scoped the
      SAME WAY (plan-p7-m3, so spec L406-410's "destructive heal
      REFUSED" has one reading: the destructive ontology actions are
      refused): source-heal still runs; the ontology section is refused
      with a loud error. Test: degraded config + `ct heal --yes` →
      source-heal mutated + ontology section refused.
- [ ] Heal-path inventory (plan-p4-MAJOR-3): the ontology pass runs ONLY
      from `ct heal` (`cmds.rs:251` `heal_run`). The GUI path
      (`run_wiki_heal` → `heal_lost_librarian_inferred` + embedding
      sweep — it does NOT call `heal_invalid_sources_conn`, comment at
      `lib.rs:2411-2414`) and the scheduler path (`heal_invalid_sources`
      at `lib.rs:709`) are both UNTOUCHED (GUI stays non-destructive;
      scheduler gains no ontology pass). Test: the ontology pass runs
      from `ct heal` only.
- [ ] Tests: §6 items 1 (heal half), 2a (degraded-load + unreadable/
      unmarked rung-1 rows), 3, 6 (watermark), 8, 9 (stale-inline-
      evidence, vault-moved, SKIP+warning, same-bytes invalidation),
      10 (empty-map-stale-hash vs off-entry scope, engine-rewrite optout
      survival r10-M3, V20 sentinel, R2.3.2a table rows).

### Task 6: Duplicate merge sweep

**Implements:** R2.7.1–R2.7.6 (incl. r8-M3 empty-summary queue,
r9-m1 re-run/Clone-demote test, r17-m3 precondition predicate, redirect
chains + cycle guard r2-m6).

- [ ] Precondition: consult the drift watermark — an unconfirmed drift
      report blocks `merge --yes` too (R2.2.8 FINAL RULE: EVERY destructive
      `--yes` action); `--confirm-drift`/`--waive-drift` accepted.
      TESTED AS `tauri_app_lib` FUNCTIONS (plan-p6-m3, per the DECISION
      RULE: the precondition predicate + drift gating are library code
      in this task; the `WikiCmd::MergeDuplicates` clap subcommand and
      its CLI rows arrive in Task 8 — Task 6 must not touch `ct.rs`).
- [ ] Report + `--yes` (never auto); BINARY survivor; grouping with
      punctuation normalization; redirect table + path compression +
      single-hop resolution with cycle guard; ONE IMMEDIATE transaction
      per merge group (Global Constraints; not one per the whole sweep);
      type-conflict and both-empty-summary queueing.
- [ ] Tests: the MERGE-SIDE rows of §6 item 4 only (read resolution,
      write resolution, transitive closure and export-as-one rows are
      Task 7).

### Task 7: Read/write redirect resolution

**Implements:** R2.7.5 read/write rules, r13-MAJOR-1 mutator rules,
r13-m3 survivor-id rule, transitive fact closure, r15-m3 reject-vs-follow
decision (follow + pin), r20-m6 `rg` audit covering all 15 files.

- [ ] Shared redirect-resolution helper; apply at every reader hit (the
      audit list); `EntityDetail.id` = survivor; archive-the-cluster;
      export maps facts through redirects (r2-M8) + exclusion test.
- [ ] Tests: §6 item 4 read/write rows + export/re-import.

### Task 8: CLI (`ct ontology`/`ct heal`/`ct wiki merge-duplicates`) + MCP

**Implements:** §2.11 (all bullets incl. the reversal rule
r13-MAJOR-2 — the `--entity` strict-writes-a-ROW grammar; plan-p9-m3:
NOT "R2.9.2's CLI half" — R2.9.2 is entirely Task 0's;
vocabulary-copy rule r13-m4, degraded-config refusal r3-M3),
the census REPORT display (query is Task 5, R2.9.3 — plan-p3-M2),
§2.10 sweep extension, §2.8 MCP catalog absence (test = §6 item 7, NOT
R2.9.2 — R2.9.2 is entirely Task 0's split-clear).

- [ ] `ct ontology set` full grammar + warnings + refusals;
      `ct heal` report DISPLAY only (flags/exit code are Task 5's —
      plan-p9-m2); one-time `ct wiki merge-duplicates`;
      `ct wiki sweep` node-type extension (census-driven).
- [ ] MCP: ontology writer NOT registered + catalog test (§2.5 MCP
      surface / §6 item 7 — NOT R2.9.2; R2.9.2's split-clear fix is Task 0).
- [ ] CI (plan-p3-M3): add the `cargo test --manifest-path
      tools/Cargo.toml -- --test-threads=1` step to `.github/workflows/ci.yml`
      (owner: this task; target job by ID, not line numbers —
      plan-p4-m7/p5-m6: add the step to job `rust-ubuntu` next to its
      "Tests + MCP integration" step; the `rust-macos` job's own "Tests"
      step is a separate matrix leg and gets nothing; there is no
      scheduled second test job). BEFORE enabling: run the FULL existing tools
      suite locally (`ct_watch_e2e`, `ct_watch_exit_codes`, watcher/
      env-dependent tests have never run in CI) — quarantine or
      `#[ignore]` environment-dependent tests with an issue link.
- [ ] Tests: §6 items 7 + §2.11 matrix cases.

### Task 9: Migration + okf_migration alignment

**Implements:** §3 migration bullets (tables; NO manifest extension —
that is the ensure's job; missing row = no-op success; incidental-off
census report-only r8-m5; single tracking issue r18-m4/delta#2),
§2.5 okf_migration abort/observability rules (r12-M2 ladder-outcome
trigger, r6-M2 loud caller, r3-M2 scoped config-fail).

- [ ] (plan-p3-M2: the R2.9.3 incidental-off census QUERY moved to Task 5
      — heal's pipeline needs it and the chain is hard; this task keeps
      only the §3 migration data work. No census bullet here.)
- [ ] okf_migration OBSERVABILITY only (plan-p2-M2: the `&mut Connection`
      signature + the `connection.rs:980` (`let mut conn`) and `:997`
      (`&mut conn`) caller fixes already landed in TASK 2 — plan-p13-m3
      corrects the earlier "Task 3" attribution; plan-p3-m4: the
      abort-vs-skip LADDER-OUTCOME decision lives in Task 3): loud
      caller log + diagnostic, retry-succeeds test.
- [ ] Tests: §6 item 6 + okf_migration cases.

### Task 10: Docs + status flips (plan-p2-M3: sweep is Task 8's SOLE deliverable)

- [ ] File THE single wave-1 tracking issue and link it from the Sept 8
      edge-integrity spec's §6 (r18-m4 / spec §3 bookkeeping — no second
      issue).
- [ ] PR body documents the §3 rollback hazard + release-notes entry
      (plan-p4-MAJOR-2 wording: NO drift report is expected after
      upgrading with an unchanged config — the initial watermark is
      stamped at first gate/heal resolution and the first-run rule
      suppresses reports with no prior row. A report appearing after
      upgrade signals a REAL config change or a vault switch that
      restored a backup carrying an older watermark — do not
      auto-waive it. Census output attached).
- [ ] Spec Status → implemented; this plan checked off; commit both with
      the PR.

## Verification (definition of done for the whole plan)

- Full CI matrix green (clippy `-D warnings`, test with
  `test-utils,mcp-server`, `--test-threads=1`; fmt stays a LOCAL
  pre-commit convention — not a CI gate, per the constraint above).
- Every §6 "minimum bar" item has a named passing test (the plan maps
  them per task; implementer asserts coverage with a checklist against
  §6 before opening the PR).
- The live-ThinkPad rollout sequence from §3 is documented in the PR
  body (migration → ensure covers manifests → heal `--yes` remap behind
  drift confirmation → one-time merge report), with the R2.9.3 census
  output attached.
