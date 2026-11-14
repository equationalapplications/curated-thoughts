# `areas`: statement delivery scoped to a named repo area

**Date:** 2026-11-14
**Status:** Draft — spec for approval. Rev 5 (2026-11-14): folds the Tier-4 delta re-gate of rev 4 (REQUEST CHANGES — 2 MAJOR / 6 MINOR, $3.5946/57 turns; every rev-4 fold confirmed held, all new cites verified live). M-A: the areas clause is PER-HUB, following the `edge_vocabulary_by_target` precedent (§6.3, §7.1); M-B: SCOPED wiring named at `tools/src/queries.rs::wisdom_match_cmd` and SCOPED is CLI-only — the sidecar cwd hazard is excluded structurally (§9.3, §17). m1–m6 folded (rule-5 cite dropped, union-of-hubs `new_name` drop, `schema_guard.rs` in §17, pin covers `ct wisdom match -- ` only, full-row rename payload precedent, `PartialEq` fixture fallout noted). Rev 4 (2026-11-14): folds the Tier-4 Fable delta review of rev 3 (REQUEST CHANGES — 5 MAJOR / 8 MINOR, $4.6886/70 turns; all 13 findings controller-verified REAL against CT main `35a51810`). M4 ruled by the owner (2026-11-14, msg `1571300634312057005`): bands are labels only, ranking unchanged; area-band priority within `--max` deferred to its own lane (§14.8). M3 adopted the controller default: bundles drop `area` like `tier`/`embed_scheme`, with a loud per-fact import report (§8.4). Rev 3 (2026-11-14): Opus round 2 returned APPROVE WITH NITS ($1.6702/78 turns); its one MINOR is folded — §9.7's provenance-blind cite corrected from `INTENT.md:198-208` (lands on rule 10) to `INTENT.md:261-263` (the provenance prohibition), controller-verified live. Rev 2 (2026-11-14): addresses Opus review round 1 (3 MAJOR / 2 MINOR, all fixed: migration is V30, not V27 — V27–V29 taken; the three-arm predicate is a NEW composable function, not a change to `entry_eligibility_where`; the rename runs through the outbox). Q14 DECIDED (owner, 2026-11-14, msg `1571117780348964967`): C-prime, the synced `area` column. Q13 RESOLVED by the record: no fourth band. Q9 CLOSED (owner, #343): no backfill. Q11 RULED: every role gets the company band. Kind drop order: owner approved an explicit deferral to the bump lane (2026-11-14, spec stop, option 1). Backed by the converged investigation of 2026-11-13/14 (two Fable consults + a strict re-gate at APPROVE WITH NITS, $4.7405/94 turns; every decisive cite controller-verified live against CT main `35a51810`). Lane spend: $45.63 ($37.13 prior investigation legs + $4.7405 strict re-gate + $1.4692 Opus round 1 + $0.6192 round-2 attempt 1 + $1.6702 Opus round 2; the rev-2 header's $46.61 double-counted the re-gate and is superseded).
**Issue:** #345.

## 1. Problem

In a prototype over one product's notes, 252 statements attached to one product node. A 2 KB brief fit 10 of 368 candidates. Two unrelated files got the same ten statements. Relevance matching alone does not narrow enough.

The ruling: a repo has a few named areas, such as "deploy", "keys" and "api". A statement attaches to the most specific place it concerns: an area, the whole repo, or the company. An agent working on a file gets the statements for that part. The agent does not get the other areas' statements.

## 2. Settled decisions (do not re-ask)

- Storage is the C-prime column (Q14). Edges and area entities are rejected (see §15).
- Q13: statements anchored outside the repo and org hubs stay out of SCOPED delivery. They stay available on request. There is no fourth band.
- Q9: no feature re-processes stored facts. Legacy rows keep their anchor and a NULL `area`.
- Q11: every role gets the company band. The single-org guard stays.
- The engine bump is its own later lane (§14). This spec reads no manifest vocabulary.
- The phrase "human-tier" is struck. The excluded class is "anchored outside the repo and org hubs".
- The spec of record (Statement of Record) stays skipped (owner: "Skip sor").
- The kind drop order is deferred to the bump lane. The deferral is explicit (§13), not silent.
- M4 (owner, 2026-11-14, msg `1571300634312057005`): bands are labels ONLY. Ranking stays weighted cosine and `--max` truncation is unchanged. The area band starts empty (no backfill, Q9); raising `--max` toward the 10 cap shows more. Area-band priority or quota within `--max` is deferred to its own lane (§14.8).

## 3. The areas file

1. Each repo keeps its areas in `.ct-areas.yaml` at the checkout root. Developers own the file. CT owns no copy of the patterns.
2. Format:
   ```yaml
   areas:
     deploy:
       - infra/**
       - amplify.yml
     keys:
       - src/kms/**
   ```
3. CT reads the file through a recorded path, not a filesystem search (the checkout requirement). The config gains a `wisdom` table — a new `WisdomConfig` struct in `config/mod.rs` (`BrainConfig` today carries `wiki`, `ingest`, `confirmations`, and others but NO `wisdom` table; `config/mod.rs:1112-1135`) — holding `wisdom.areas_paths`: a map from repo entity ID to the absolute areas-file path (entity ids are unique within a brain — `curated_entities.id` is the primary key — so the map cannot collide), and `wisdom.org_entity_id`: the org hub's entity id (§9.3). The owner records both once per machine.
4. The Librarian's area names come from this file. A synced names list is deferred (see §14). No checkout path for a hub means UNSCOPED mode for that hub (§4).
5. CT reads the file at match time. CT re-reads when the file's mtime changes.

## 4. Modes

1. **SCOPED.** The hub has an areas file and the query carries `--path=`. CT maps the path to an area and returns three bands: the area's statements, the whole-repo statements, and the company statements. The bands are LABELS ONLY (M4 ruling, §2): ranking stays weighted cosine and `--max` truncation is unchanged. Day one the area band is empty (no backfill, §2 Q9), so SCOPED equals the repo-hub plus org-hub facts; raising `--max` toward the 10 cap (`MAX_ENTRIES`, `wisdom_match.rs:41`) shows more.
2. **UNSCOPED.** No areas file, or the query carries no `--path=`. CT returns today's behavior, byte-identical (§9, the backstop test).
3. **Mis-scoped.** A stored fact carries an area name the areas file does not define. Ingest rejects it and reports it (§6). It never lands silently as whole-repo.
4. A path that matches no pattern returns the repo and company bands only. This is a normal outcome, not an error. A branch that has moved files before CT saw it behaves the same way. The issue accepts this case as rare.
5. Band selection without a `--path=` is deferred to CTI#96 item 4 (explicit, §14).

## 5. Storage: the `area` column

1. Add migration V30: `ALTER TABLE llm_wiki_entries ADD COLUMN area TEXT NULL;` (the latest migration today is V29 — V27 is the embed-scheme stamp `db/connection.rs:829`, V28 is `documents.embed_key` `:1116`, V29 `:1321`; CT wires new migrations in `db/connection.rs`, so V30 lands there with the others).
2. The column is CT-local, added by CT's own migration. This follows the `tier` precedent (`db/schema.rs:363`) and the `embed_scheme` precedent (`db/connection.rs:1065`).
3. The DDL mirror gate is unaffected: `package_ddl_matches_rust` (`db/ddl_compat.rs:42`) compares the package DDL only. CT-local columns live outside that comparison.
4. The column holds the area name as spelled in the areas file. The name is self-describing on any machine. NULL means whole-repo.
5. Membership is the pair (`entity_id`, `area`). There is no membership edge, no area entity, and no `partOf`.
6. The column rides the entries replication path (INTENT rule 5). Edges are deliberately not replicated (`db/edge_purge.rs:9`, asserting test `db/commit.rs:4474-4482`), which is one reason membership must not be an edge. The engine's `applyOntologyChange` deletes every edge row for an entity (`src/lib/wiki.ts:107-110`); the column survives it.
7. The new-fact INSERT (`db/commit.rs:1697-1702`) gains the `area` column and its bind parameter.

## 6. Writing the area: a CT-local fact field

1. The Librarian's fact item gains an optional `area` field. This is a CT-local proposal field with its own prompt clause. It is NOT manifest vocabulary: neither the pinned 8.0.2 nor the installed 7.12.0 schema package declares an `area` type (verified 2026-11-14 — see §14.1), so an `about → area` edge item is not nameable to the model, and every area item would land in `dropped_edges`. The band would be empty on day one.
2. `LlmFact` is a private struct whose fields all carry `#[serde(default)]` (`librarian/synthesis.rs:91-103`); it gains `#[serde(default)] area: Option<String>`. The `fact_add` payload builder gains the field (`synthesis.rs:947-951`). The prompt schema string gains the field (`synthesis.rs:622`).
3. When ANY configured hub has an areas file, the built system prompt carries PER-HUB areas clauses, following the code's own per-target precedent: the prompt builder already takes a per-target map (`build_system_prompt(mode, edge_vocabulary, edge_vocabulary_by_target, node_vocabulary)`, `librarian/synthesis.rs:660-665`) and emits one line per target — `For <target>: use ONLY these edge_type values…` (`:693-698`). The areas clause uses the same form: one `For <hub entity id>: areas = …` line per configured hub, listing that hub's names as a fixed list. A single merged list is FORBIDDEN: two repos can define the same name (`deploy` in both), and a merged list cannot tell the model which hub's name it means. `area` on a proposal whose target is not a configured hub — including every `New` target (`:911-916`, which carries no hub id) — is rejected per §6.6. The model cannot invent near-duplicates such as "deploy" and "deployment".
4. The clause states the coupling rule: membership is expressed only through the fact field, never through an edge item.
5. **The `new_name` danger and its test.** The edge `new_name` branch binds by name against `live_entities` of ANY type, for EVERY edge type, case-insensitively, brain-wide (`db/commit.rs:1601-1612` — exact-case wins, otherwise NOCASE, ties break on lowest id; no per-target restriction), and a `New`-target proposal has no repo at validation time. Once the prompt advertises area names, a model-emitted edge of ANY type whose `new_name` equals an area name (for example `relatesTo → {new_name: "Deploy"}`) would bind to a same-named entity. Therefore: the writer drops ANY edge item whose `new_name` equals an area name of ANY CONFIGURED HUB — the UNION of all hubs' names across the `wisdom.areas_paths` map — into `dropped_edges`, compared NOCASE, with a reason. The negative test asserts that drop.
6. A model proposal whose `area` does not resolve against the areas file is rejected and reported, in the `dropped_edges` house style. It never lands silently as whole-repo.
7. **Known gap: cross-machine `area` divergence.** The areas clause (§6.3) and the §6.6 rejection run only where `wisdom.areas_paths` is configured. INTENT rule 5 gives two machines the same content-derived fact id for the same file; a machine with the area configured stamps `area`, one without stamps NULL, and both emit Insert payloads for the same id with different `area`. Which value wins is undefined in this spec. Recorded as a known gap, closed by the synced names list (§14.7), which makes the areas configuration itself replicable.

## 7. The three-layer acceptance test

A commit-only guard test passes even when the model can never emit `area`. The acceptance test must span all three layers:

1. The built system prompt contains the PER-HUB areas clause (§6.3): one `For <hub entity id>:` line per configured hub, carrying that hub's names — the test asserts the per-hub form, NOT a merged list.
2. A model output carrying `area` on a fact item parses into a `fact_add` payload carrying `area`.
3. The commit sets the column and the outbox payload carries it.

## 8. Outbox and the receive mechanism

1. `wiki_fact_outbox_payload` (`db/commit.rs:1249`) gains `area: Option<&str>`, emitted in the JSON payload.
2. There are exactly SIX call sites; all gain the argument: `db/commit.rs:1740`, `db/commit.rs:1960`, `db/wisdom.rs:499`, `db/drafts.rs:69`, `db/restore_sync.rs:596`, `db/bundle_apply.rs:517`.
3. The payload intentionally exceeds the engine `WikiFact` shape. The schema guard records `area` as a named CT-local divergence, like `tier` and `embed_scheme`.
4. **The receive mechanism, named.** The payload travels as `OutboxEvent.payload` JSON to the configured sink (`outbox/mod.rs:14-29`, `OutboxConfig`; the `Sink` trait at `:46`). Receiving machines read entries from the remote database (INTENT rule 5). CT has no other inbound applier today. Bundles drop `area` the way they drop `tier` and `embed_scheme` (both listed CT-local in the schema guard, `schema_guard.rs:53-56`): the export loads facts into the engine `WikiFact` shape, which has no `area` (`bundle_io.rs:85-132`), and the import INSERT sets none (`bundle_apply.rs:476-511`). To keep §4.3's promise audible, bundle import prints a loud per-fact report line stating that the fact arrived without an area and lands whole-repo. Carrying `area` in bundles is new scope, not taken here.
5. Tests: the built payload JSON carries `area`. A round trip through `sync_batch` passes the payload through unchanged. The sink receives `OutboxEvent.payload` containing `area`.

## 9. The delivery predicate: three arms, every arm repo-bound

1. `gated_entries` is brain-wide (`wisdom_match.rs:541-547`). Every arm must bind the repo or org hub. A fragment that omits the hub cross-matches same-named areas across repos.
2. The three arms are a NEW composable function `band_predicate_where(bindings: &BandBindings) -> String` in `wisdom_match.rs`. The existing `entry_eligibility_where` (`wisdom_match.rs:470`, signature `(t: &Temporal) -> String`) and its consumers (`:502`, `:547`, `two_stage.rs:96`, `:132`) stay untouched. SCOPED calls compose the two: temporal eligibility, then the band predicate. UNSCOPED callers pass no bindings and run today's SQL. Single source of truth: the three arms exist only in the new function. No other file changes for the predicate: every two-stage result flows through `gated_entries`' `id_filter` (`wisdom_match.rs:905`, `:949`, `:973`), so enforcing the band predicate in `gated_entries` alone covers the two-stage path — `two_stage.rs` stays untouched.
   - area arm: `e.entity_id = :repo AND e.area = :area`
   - repo arm: `e.entity_id = :repo AND e.area IS NULL`
   - org arm: `e.entity_id = :org AND e.area IS NULL`
3. **The path resolver, specified.** `--path=` is passed through verbatim today (`tools/src/bin/ct.rs:852-865`; existing tests pass relative values, `tools/tests/ct_wisdom_match.rs:288`); the resolver is a NEW layer on top and never edits the target string itself. It resolves the path against `cwd` when relative, then binds the hub whose checkout root (the areas file's directory, §3.1) is the LONGEST PREFIX of the resolved absolute path — nesting resolves to the innermost checkout, and a tie is impossible because prefix lengths differ; the map keys are entity ids and cannot collide (§3.3). When no configured checkout prefixes the path, the resolver contributes nothing and the query stays byte-identical to today's activity call (§9.8). **SCOPED is a CLI-only surface in this spec.** The resolver's `cwd` rule is sound for the one-shot `ct` process, whose cwd is the agent's; the MCP sidecar's cwd is the sidecar's own, so a relative `--path=` there would bind the wrong hub or none — silently. No sidecar surface is SCOPED in this spec; if a later lane scopes one, it REQUIRES an absolute `--path=` and states so. The org hub comes from `wisdom.org_entity_id` (§3.3): the config carries exactly ONE org entity id, so the single-org guard is structural in CT. At match start the resolver verifies the configured org entity exists and is live; if it does not, the org arm is OMITTED (the §9.4 form) and the deferred check command (§14.5) reports the misconfiguration.
4. In SCOPED mode with no area resolved, the area arm is OMITTED, not bound to NULL. Omission is the form the re-gate verified.
5. The stage-1 complement needs no change of its own: its candidates flow through `gated_entries` (`two_stage.rs:96`, `:132`), where the band predicate is enforced (§9.2).
6. Corrections are band-independent: `find_corrections` is exclude-driven (`wisdom_match.rs:1368-1398`). CTI rule 3 depends on this. Pin it with a test.
7. The predicate is provenance-blind: two fixtures differing only in `source_type` produce identical SCOPED output (CT#344; `INTENT.md:261-263`).
8. **Backstop tests, both arms named.** Message-mode: the byte-pin test lives at `tool_dispatch.rs:2189` (`message_mode_output_byte_identical`, which runs `wisdom_match_with_floor` in message mode only, `:2229-2238`). It covers the message-mode `ct wisdom match -- <text>` output ONLY — NOT `ct recall`: `recall_cmd` calls `rank_wiki_entries`, never `wisdom_match_with_floor` (`tools/src/queries.rs:446-452`), and the pin's part (a) merely checks additive keys on that ranking path (`tool_dispatch.rs:2215-2224`). `ct recall` is outside this spec's changed path anyway: the band predicate lives in `gated_entries`, which `ct recall` does not traverse. Activity-mode is the arm SCOPED changes — it fires only on `--path=`, which maps to `QueryKind::Activity` (`ct.rs:854-855`) and cannot combine with a message after `--` ("exactly one target", `ct.rs:842-850`) — so the message pin never executes it. Its UNSCOPED backstop is the four existing activity pins: `activity_no_flag_fallback_is_byte_identical_to_the_message_call` (`tools/tests/ct_wisdom_match.rs:359`), the two-stage arm (`:378`), the empty-target/max-0 arm (`:409`), and the gate-label test (`:427`). This spec adds one NEW pin: with an areas file configured for a DIFFERENT repo — or a `--path=` outside every configured checkout — the output stays byte-identical to the message call.

## 10. Per-item `band` and `area` output fields

1. `WisdomItem` gains `band: Option<String>` and `area: Option<String>` (`wisdom_match.rs:134-143`; today: id, title, text, score, supersedes, provenance).
2. Both fields carry `#[serde(skip_serializing_if = "Option::is_none")]`. A plain `Option` serializes `null` on every item and breaks the byte pin.
3. Both fields are emitted only in SCOPED mode. Corrections items omit both.
4. The pin comment at `tool_dispatch.rs:2189` is updated to name this exception.
5. Purpose: the axon#48 dispatcher labels `ct:<area>` guidance without re-deriving the match.
6. `band` holds `"area"`, `"repo"`, or `"org"`. `area` holds the name for area-band items and is absent otherwise.
7. **Test fallout, budgeted.** `WisdomItem` derives `PartialEq` (`wisdom_match.rs:134`), so the two new fields break every struct-literal fixture that constructs the item — the plan budgets that mechanical update. Because the fields carry `skip_serializing_if = "Option::is_none"` (§10.2), the `message_mode_output_byte_identical` fixture STRING itself stays unchanged: message-mode items carry no `band`/`area` and serialize to the same bytes.

## 11. Ingest attach and measurement (work items)

1. The Librarian prompt gains an attach clause: a statement attaches through the thing it concerns. Nothing else steers a new statement toward the owning app or org today (`librarian/synthesis.rs:899-917`; the prompt "self" is the proposal's target, `:629`).
2. **Measured attach rate.** A test measures the attach rate on a fixture. The spec claims no rate in advance. The #381 lesson stands: prompt steering produced 0 of 28.
3. **Backlog count.** A one-time count runs on a scratch brain: backlog by anchor type and provenance. The spec discloses the numbers.
4. No backfill (owner ruling, #343). Legacy rows keep their anchor and a NULL `area`.

## 12. Renames

1. The actor is the owner. The owner runs `ct wisdom areas rename <old> <new>` against a hub whose areas file defines `new`.
2. The rename runs through the commit/outbox path, NOT a bare UPDATE. One transaction: `UPDATE llm_wiki_entries SET area = :new WHERE area = :old AND entity_id = <repo hub>`, then `push_entries_outbox` (`db/commit.rs:1342`) emits one outbox Update row per affected entry. The Update payload is a FULL-ROW payload — `wiki_fact_outbox_payload` takes the row's fields (`db/commit.rs:1249-1271`) — so the rename re-SELECTs each affected row to build it; `reassert_entries` (`db/restore_sync.rs:534-591`) is the exact payload-building precedent, and that SELECT gains `area` (§17). A bare UPDATE would replicate nothing: remote machines would keep the old name and SCOPED delivery would silently break there.
3. This is a direct column rewrite. It does not collide with the #343 no-reprocessing ruling: nothing re-extracts or re-processes facts.
4. Until the rename runs, stale-name rows match no arm and drop out of SCOPED delivery (accepted; see §14 for the check command deferral).
5. **INTENT rule 1 reconciliation.** Rule 1 forbids agent tools from adding, changing, approving, or archiving FACTS in the database (`INTENT.md:82-84`). `area` is delivery-scoping metadata, not fact content: the rename touches no body, tag, evidence, or provenance — it re-scopes delivery only, it is owner-invoked rather than an agent surface, and the INTENT glossary's agent-tool concern is fact content, which this command does not write. The command still makes the actor auditable: it stamps the caller from `os_account()` (`db/proposals_review.rs:72`, the shared review-surface precedent) and writes one audit row per renamed run, in the same vault-mirrored house style as `stated/.confirmations.jsonl`.
6. **`updated_at` moves.** The rename bumps `updated_at` on every renamed row: the outbox Update payload carries `updated_at` as the change token a remote applies, and a rename that left it stale would lose to last-write ordering. The content-derived fact id does not change.

## 13. Kind storage: the deferral, made explicit

The new-fact INSERT sets no `okf_type` (`db/commit.rs:1697-1702`). The budget drop order needs kinds. This spec defers the kind drop order to the bump lane. The deferral is explicit here, not a silent open question. Axon's drop-lowest-ranked behavior inherits the deferral until kinds exist. Owner approved this deferral at the 2026-11-14 spec stop.

## 14. Out of scope

1. **The engine bump lane.** Triggers: (a) a feature needs 9.0.0 engine behavior; (b) a traversal need for `partOf` or `area` entities. Cost items, cites verified: four packages at 8.0.2 plus overrides plus `PINNED_CORE_LLM_WIKI_VERSION` (`schema_guard.rs:8`); the `vector_removals` DDL mirror gate; the `EA_SEED_TYPES` redesign (`entity_gate.rs:841`; foreign-manifest declare-or-report fallback `:895-911` — the `:857-879` stretch is the EA-subset extension branch; mirror moved to `connection.rs:4669`); heal for handoff and session_recap rows; `ontology-packages.test.ts` at `src/__tests__/ontology-packages.test.ts` breaks (17/40 → 21/42; the pinned/installed wording: pinned 8.0.2, installed 7.12.0 — and NEITHER declares `area`, verified 2026-11-14 by a whole-word sweep of the published `@equationalapplications/schema-software-org@8.0.2` npm tarball: `person`/`project`/`concept` present in the embedded manifest, zero occurrences of `area`; the installed 7.12.0 tree sweeps clean the same way).
2. **The bump lane inherits a REQUIRED decision.** Once a 9.0.0 manifest is seeded (new brain or `applyOntologyChange`, `src/lib/wiki.ts:130`), the node clause advertises `area`, and the model may create area entities and edges alongside the column. The bump lane must then decide the ingest mapping: map area items to the column, or drop them. Deferring this decision is what prevents two representations from coexisting.
3. **The manifest-only cheaper split needs verification in the bump lane.** The installed schema package declares a hard `dependencies` entry on core-llm-wiki (`package.json:43-44`), not a types-only import. The split works only if type imports stay compatible.
4. #381 holds no coupling under C-prime. Re-check only if CT adopts the engine classifier.
5. **The areas check command** (report files matching no area, patterns matching nothing, and area names the brain uses that the file does not define) is deferred to its own small lane. The curated-thoughts-integrations skill will tell the agent when to run it.
6. **Path-less band selection** is deferred to CTI#96 item 4.
7. **A synced names list** (area names without a checkout) is deferred. Version one requires the checkout path (§3.3).
8. **Area-band priority or quota within `--max`** is deferred to its own lane. It is built only after real area data exists, and it must come with a benchmark showing area items are being crowded out. (Owner ruling, 2026-11-14, msg `1571300634312057005`.)

## 15. Rejected alternatives

1. **Edge-based membership.** Edges do not replicate (`edge_purge.rs:9`; asserting test `commit.rs:4474-4482`), so pure-edge scoping breaks INTENT rule 5 across machines. The engine wipes an entity's edges on every ontology change (`src/lib/wiki.ts:107-110`). Under the pinned 8.0.2 / installed 7.12.0 manifest an area edge item is unnameable (§14.1, the 8.0.2 sweep), so the band is empty on day one.
2. **Dual representation** (edges plus column): drift. The engine review rejected it as "dual".
3. **Manifest vocabulary for area**: no `area` type exists in the pinned 8.0.2 or the installed 7.12.0 schema package (§14.1). The band would be empty on day one.
4. **Disclose mode, E-anchor, T-tag**: the tag route fails because `commit_fact_update` overwrites `tags` wholesale.
5. **A synced names list without a checkout** (version one): deferred (§14.7).

## 16. Engine ↔ CT mapping

| engine `about` target | CT storage |
|---|---|
| `about → area` (repo + name) | `entity_id = repo, area = name` |
| `about → app` (repo) | `entity_id = repo, area IS NULL` |
| `about → org` (org) | `entity_id = org, area IS NULL` |

## 17. Files this spec touches

- `db/connection.rs`: migration V30 body and wiring (alongside V27–V29; older CT migrations live in `schema.rs`, e.g. `tier` at `:363`).
- `db/commit.rs`: INSERT gains the column; payload builder; two call sites; the `new_name` drop.
- `db/wisdom.rs`, `db/drafts.rs`: one payload call site each.
- `db/restore_sync.rs`: the row SELECT (`:536-541`) gains `area`, plus its payload call site (`:596`).
- `db/bundle_apply.rs`: its payload call site (`:517`) gains the argument (passing NULL for bundle-borne facts) plus the loud per-fact import report (§8.4). `bundle_io.rs` and the INSERT stay unchanged — bundles keep dropping `area` (§8.4).
- `librarian/synthesis.rs`: `LlmFact.area`, the payload builder, the schema string, the areas clause, the attach clause.
- `wisdom_match.rs`: `WisdomItem` fields; the NEW `band_predicate_where` function and its three arms; the corrections pin.
- `two_stage.rs`: NO change (§9.2, §9.5).
- `tools/src/queries.rs` (`wisdom_match_cmd`, `:824`): the SCOPED wiring AND the resolver site — `ct.rs:866` is the caller. `tool_dispatch.rs` holds ONLY the byte pin, so its change reduces to the pin-comment update (§10.4).
- `db/schema_guard.rs`: the guard records `area` as a named CT-local divergence beside `tier`/`embed_scheme` (§8.3).
- `tool_dispatch.rs`: the pin-comment update only (§10.4).
- `config/`: the `WisdomConfig` struct and TOML `wisdom` table — `areas_paths` (repo entity id → absolute areas-file path) and `org_entity_id` (§3.3, §9.3).
- CLI: `ct wisdom match --path=`; `ct wisdom areas rename <old> <new>` (through `push_entries_outbox`, with the §12.5 actor stamp and audit row).
- Tests per §7, §8.5, §9, §10, §11.

## 18. Conventions of record

- Spec first: `INTENT.md` "How changes are made" step 1 (`INTENT.md:268-271`). Spec, plan, and implementation travel one PR (#349/#350 precedent). The PR opens as a draft titled "spec for approval"; the owner's review gates the merge.
- Recall and injection gates: no superseded fact is ever injected; cached on-request output stays byte-identical (`INTENT.md` step 4).
- Facts stay inside the project they came from (`INTENT.md` rule 8). Areas narrow further inside a project. They never widen.
- Cites name CT main `35a51810` (v3.13.0). Line numbers may shift; reviewers re-locate by symbol name if a line moves.

## 19. Benchmark gates

1. The INTENT step-4 benchmark gates (no superseded fact injected; LongMemEval hit rate and abstention precision; paraphrase; exactly-once) apply to the UNSCOPED path. The concrete gates are the backstop tests of §9.8: the message-mode `ct wisdom match -- ` byte pin (`tool_dispatch.rs:2189`), the four UNSCOPED activity pins (`tools/tests/ct_wisdom_match.rs:359/:378/:409/:427`) for the `--path=` arm, and the new outside-every-checkout pin. `ct recall` is outside the changed path (§9.8). UNSCOPED behavior must not move.
2. SCOPED delivery is NEW behavior with no prior baseline. There is nothing to regress against. Its gates are the tests this spec names: the three-layer acceptance test (§7), the band-predicate tests (§9), the provenance-blind test (§9.7), and the corrections pin (§9.6).
3. The spec claims no attach rate in advance (§11.2). The measured result lands in the plan, not here.
