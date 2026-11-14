# `areas`: statement delivery scoped to a named repo area

**Date:** 2026-11-14
**Status:** Draft — spec for approval. Rev 2 (2026-11-14): addresses Opus review round 1 (3 MAJOR / 2 MINOR, all fixed: migration is V30, not V27 — V27–V29 taken; the three-arm predicate is a NEW composable function, not a change to `entry_eligibility_where`; the rename runs through the outbox). Q14 DECIDED (owner, 2026-11-14, msg `1571117780348964967`): C-prime, the synced `area` column. Q13 RESOLVED by the record: no fourth band. Q9 CLOSED (owner, #343): no backfill. Q11 RULED: every role gets the company band. Kind drop order: owner approved an explicit deferral to the bump lane (2026-11-14, spec stop, option 1). Backed by the converged investigation of 2026-11-13/14 (two Fable consults + a strict re-gate at APPROVE WITH NITS, $4.7405/94 turns; every decisive cite controller-verified live against CT main `35a51810`). Lane spend: $46.61.
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
3. CT reads the file through a recorded path, not a filesystem search (the checkout requirement). The config gains `wisdom.areas_paths`: a map from repo entity name to the absolute areas-file path. The owner records the path once per repo checkout.
4. The Librarian's area names come from this file. A synced names list is deferred (see §14). No checkout path for a hub means UNSCOPED mode for that hub (§4).
5. CT reads the file at match time. CT re-reads when the file's mtime changes.

## 4. Modes

1. **SCOPED.** The hub has an areas file and the query carries `--path=`. CT maps the path to an area and returns three bands: the area's statements, the whole-repo statements, and the company statements.
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

1. The Librarian's fact item gains an optional `area` field. This is a CT-local proposal field with its own prompt clause. It is NOT manifest vocabulary: the installed 8.0.2 manifest has no `area` type, so an `about → area` edge item is not nameable to the model, and every area item would land in `dropped_edges`. The band would be empty on day one.
2. `LlmFact` gains `pub area: Option<String>` (`librarian/synthesis.rs:91-103`; today it has none). The `fact_add` payload builder gains the field (`synthesis.rs:947-951`). The prompt schema string gains the field (`synthesis.rs:622`).
3. When the target hub has an areas file, the built system prompt carries an areas clause. The clause lists the file's area names as a fixed list. The model cannot invent near-duplicates such as "deploy" and "deployment".
4. The clause states the coupling rule: membership is expressed only through the fact field, never through an edge item.
5. **The `new_name` danger and its test.** The edge `new_name` branch binds by name against `live_entities` of ANY type (`db/commit.rs:1601-1610`). Once the prompt advertises area names, a model-emitted `about → {new_name: "deploy"}` edge would bind to any same-named entity. Therefore: the writer drops an `about` edge whose `new_name` equals an area name of the target repo into `dropped_edges`, with a reason. The negative test asserts that drop.
6. A model proposal whose `area` does not resolve against the areas file is rejected and reported, in the `dropped_edges` house style. It never lands silently as whole-repo.

## 7. The three-layer acceptance test

A commit-only guard test passes even when the model can never emit `area`. The acceptance test must span all three layers:

1. The built system prompt contains the areas clause with the fixture file's names.
2. A model output carrying `area` on a fact item parses into a `fact_add` payload carrying `area`.
3. The commit sets the column and the outbox payload carries it.

## 8. Outbox and the receive mechanism

1. `wiki_fact_outbox_payload` (`db/commit.rs:1249`) gains `area: Option<&str>`, emitted in the JSON payload.
2. There are exactly SIX call sites; all gain the argument: `db/commit.rs:1740`, `db/commit.rs:1960`, `db/wisdom.rs:499`, `db/drafts.rs:69`, `db/restore_sync.rs:596`, `db/bundle_apply.rs:517`.
3. The payload intentionally exceeds the engine `WikiFact` shape. The schema guard records `area` as a named CT-local divergence, like `tier` and `embed_scheme`.
4. **The receive mechanism, named.** The payload travels as `OutboxEvent.payload` JSON to the configured sink (`outbox/mod.rs:14-29`, `OutboxConfig`; the `Sink` trait at `:46`). Receiving machines read entries from the remote database (INTENT rule 5). CT has no other inbound applier today.
5. Tests: the built payload JSON carries `area`. A round trip through `sync_batch` passes the payload through unchanged. The receiving side reconstructs the row with the field.

## 9. The delivery predicate: three arms, every arm repo-bound

1. `gated_entries` is brain-wide (`wisdom_match.rs:541-547`). Every arm must bind the repo or org hub. A fragment that omits the hub cross-matches same-named areas across repos.
2. The three arms are a NEW composable function `band_predicate_where(bindings: &BandBindings) -> String` in `wisdom_match.rs`. The existing `entry_eligibility_where` (`wisdom_match.rs:470`, signature `(t: &Temporal) -> String`) and its consumers (`:502`, `:547`, `two_stage.rs:96`, `:132`) stay untouched. SCOPED calls compose the two: temporal eligibility, then the band predicate. UNSCOPED callers pass no bindings and run today's SQL. Single source of truth: the three arms exist only in the new function.
   - area arm: `e.entity_id = :repo AND e.area = :area`
   - repo arm: `e.entity_id = :repo AND e.area IS NULL`
   - org arm: `e.entity_id = :org AND e.area IS NULL`
3. The path resolver maps `--path=` to the repo hub and the area name. It binds BOTH. It resolves patterns against the areas file.
4. In SCOPED mode with no area resolved, the area arm is OMITTED, not bound to NULL. Omission is the form the re-gate verified.
5. The stage-1 complement inherits the same `band_predicate_where` function.
6. Corrections are band-independent: `find_corrections` is exclude-driven (`wisdom_match.rs:1368-1398`). CTI rule 3 depends on this. Pin it with a test.
7. The predicate is provenance-blind: two fixtures differing only in `source_type` produce identical SCOPED output (CT#344; `INTENT.md:198-208`).
8. **Backstop test.** Without an areas file, the message-mode outputs of `ct wisdom match` and `ct recall` stay byte-identical for the same seeded brain. The byte-pin test lives at `tool_dispatch.rs:2189` (`message_mode_output_byte_identical`).

## 10. Per-item `band` and `area` output fields

1. `WisdomItem` gains `band: Option<String>` and `area: Option<String>` (`wisdom_match.rs:134-143`; today: id, title, text, score, supersedes, provenance).
2. Both fields carry `#[serde(skip_serializing_if = "Option::is_none")]`. A plain `Option` serializes `null` on every item and breaks the byte pin.
3. Both fields are emitted only in SCOPED mode. Corrections items omit both.
4. The pin comment at `tool_dispatch.rs:2189` is updated to name this exception.
5. Purpose: the axon#48 dispatcher labels `ct:<area>` guidance without re-deriving the match.
6. `band` holds `"area"`, `"repo"`, or `"org"`. `area` holds the name for area-band items and is absent otherwise.

## 11. Ingest attach and measurement (work items)

1. The Librarian prompt gains an attach clause: a statement attaches through the thing it concerns. Nothing else steers a new statement toward the owning app or org today (`librarian/synthesis.rs:899-917`; the prompt "self" is the proposal's target, `:629`).
2. **Measured attach rate.** A test measures the attach rate on a fixture. The spec claims no rate in advance. The #381 lesson stands: prompt steering produced 0 of 28.
3. **Backlog count.** A one-time count runs on a scratch brain: backlog by anchor type and provenance. The spec discloses the numbers.
4. No backfill (owner ruling, #343). Legacy rows keep their anchor and a NULL `area`.

## 12. Renames

1. The actor is the owner. The owner runs `ct wisdom areas rename <old> <new>` against a hub whose areas file defines `new`.
2. The rename runs through the commit/outbox path, NOT a bare UPDATE. One transaction: `UPDATE llm_wiki_entries SET area = :new WHERE area = :old AND entity_id = <repo hub>`, then `push_entries_outbox` (`db/commit.rs:1342`) emits one outbox Update row per affected entry. A bare UPDATE would replicate nothing: remote machines would keep the old name and SCOPED delivery would silently break there.
3. This is a direct column rewrite. It does not collide with the #343 no-reprocessing ruling: nothing re-extracts or re-processes facts.
4. Until the rename runs, stale-name rows match no arm and drop out of SCOPED delivery (accepted; see §14 for the check command deferral).

## 13. Kind storage: the deferral, made explicit

The new-fact INSERT sets no `okf_type` (`db/commit.rs:1697-1702`). The budget drop order needs kinds. This spec defers the kind drop order to the bump lane. The deferral is explicit here, not a silent open question. Axon's drop-lowest-ranked behavior inherits the deferral until kinds exist. Owner approved this deferral at the 2026-11-14 spec stop.

## 14. Out of scope

1. **The engine bump lane.** Triggers: (a) a feature needs 9.0.0 engine behavior; (b) a traversal need for `partOf` or `area` entities. Cost items, cites verified: four packages at 8.0.2 plus overrides plus `PINNED_CORE_LLM_WIKI_VERSION` (`schema_guard.rs:8`); the `vector_removals` DDL mirror gate; the `EA_SEED_TYPES` redesign (`entity_gate.rs:841`; foreign-manifest fallback `:857-879`; mirror moved to `connection.rs:4669`); heal for handoff and session_recap rows; `ontology-packages.test.ts` at `src/__tests__/ontology-packages.test.ts` breaks (17/40 → 21/42; the pinned/installed wording: pinned 8.0.2, installed 7.12.0).
2. **The bump lane inherits a REQUIRED decision.** Once a 9.0.0 manifest is seeded (new brain or `applyOntologyChange`, `src/lib/wiki.ts:130`), the node clause advertises `area`, and the model may create area entities and edges alongside the column. The bump lane must then decide the ingest mapping: map area items to the column, or drop them. Deferring this decision is what prevents two representations from coexisting.
3. **The manifest-only cheaper split needs verification in the bump lane.** The installed schema package declares a hard `dependencies` entry on core-llm-wiki (`package.json:43-44`), not a types-only import. The split works only if type imports stay compatible.
4. #381 holds no coupling under C-prime. Re-check only if CT adopts the engine classifier.
5. **The areas check command** (report files matching no area, patterns matching nothing, and area names the brain uses that the file does not define) is deferred to its own small lane. The curated-thoughts-integrations skill will tell the agent when to run it.
6. **Path-less band selection** is deferred to CTI#96 item 4.
7. **A synced names list** (area names without a checkout) is deferred. Version one requires the checkout path (§3.3).

## 15. Rejected alternatives

1. **Edge-based membership.** Edges do not replicate (`edge_purge.rs:9`; asserting test `commit.rs:4474-4482`), so pure-edge scoping breaks INTENT rule 5 across machines. The engine wipes an entity's edges on every ontology change (`src/lib/wiki.ts:107-110`). Under the installed 8.0.2 manifest an area edge item is unnameable, so the band is empty on day one.
2. **Dual representation** (edges plus column): drift. The engine review rejected it as "dual".
3. **Manifest vocabulary for area**: no `area` type exists at 8.0.2. The band would be empty on day one.
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
- `db/wisdom.rs`, `db/drafts.rs`, `db/restore_sync.rs`, `db/bundle_apply.rs`: one call site each.
- `librarian/synthesis.rs`: `LlmFact.area`, the payload builder, the schema string, the areas clause, the attach clause.
- `wisdom_match.rs`: `WisdomItem` fields; the NEW `band_predicate_where` function and its three arms; the corrections pin.
- `tool_dispatch.rs`: SCOPED wiring; the pin comment update.
- `config/`: the `wisdom.areas_paths` key.
- CLI: `ct wisdom match --path=`; `ct wisdom areas rename <old> <new>` (through `push_entries_outbox`).
- Tests per §7, §8.5, §9, §10, §11.

## 18. Conventions of record

- Spec first: `INTENT.md` "How changes are made" step 1 (`INTENT.md:268-271`). Spec, plan, and implementation travel one PR (#349/#350 precedent). The PR opens as a draft titled "spec for approval"; the owner's review gates the merge.
- Recall and injection gates: no superseded fact is ever injected; cached on-request output stays byte-identical (`INTENT.md` step 4).
- Facts stay inside the project they came from (`INTENT.md` rule 8). Areas narrow further inside a project. They never widen.
- Cites name CT main `35a51810` (v3.13.0). Line numbers may shift; reviewers re-locate by symbol name if a line moves.

## 19. Benchmark gates

1. The INTENT step-4 benchmark gates (no superseded fact injected; LongMemEval hit rate and abstention precision; paraphrase; exactly-once) apply to the UNSCOPED path. The concrete gate is the byte-identical backstop test (§9.8): UNSCOPED behavior must not move.
2. SCOPED delivery is NEW behavior with no prior baseline. There is nothing to regress against. Its gates are the tests this spec names: the three-layer acceptance test (§7), the band-predicate tests (§9), the provenance-blind test (§9.7), and the corrections pin (§9.6).
3. The spec claims no attach rate in advance (§11.2). The measured result lands in the plan, not here.
