# Spec: Ontology Activation, Entry Tier Dimension, Composite Context Primitive

Baseline: v1.40.1 (`d31c208`, main). Scope: **Curated Thoughts codebase only** —
general-purpose features for any deployment using an immutable source archive +
curated-wisdom memory workflow. The motivating deployment (Equational Applications'
agent memory) has its own deployment spec in that project's private vault and is
referenced here only as context. Sibling lineage:
2026-08-31-wiki-graph-reanchor-entry-embeddings-design.md (PR #131) and
2026-09-01-wiki-reader-contract-and-forget-outbox-design.md (PR #135, v1.40.1).
Docs ride the implementing PR (Kurt directive, Aug 31 2026).

## 1. Executive Summary & Problem Context

CT deployments that ingest an immutable source archive and maintain a curated
wisdom layer currently have three gaps between the curation model and the
schema/tools:

- **G1 — Recorded ontology selection never seeds the manifest.** `config.json`
  records `ontology.schema` (e.g. `schema-software-org`), but
  `wiki_get_ontology` returns `{"manifest":null,"mode":"off"}` on such brains.
  Rust owns the *selection* only; the TypeScript engine seeds the manifest from
  the npm schema package via `createWiki` (strict-schema adoption spec,
  2026-08-28). Deployments onboarded before that path existed (or where the seed
  never ran) are stuck untyped. Folders/edges stay semantic by convention.
- **G2 — Tier is not a stored dimension.** `llm_wiki_entries` has no tier
  column. The fact/wisdom distinction exists only as prompt labels over
  `chunk.tier` (`librarian/mod.rs` `assemble_librarian_context`: `user_doc` →
  `tier_fact` "ANCHOR TRUTH — do not propose modifications"; `tier_wisdom` →
  "CURATED WISDOM — may be updated via Wisdom proposals"). The curated layer is
  not queryable as such; the tier namespaces the prompt vocabulary implies
  (`tier_fact`/`tier_wisdom` — the same names at the center of #133) have no
  stored counterpart.
- **G3 — GraphRAG retrieval requires namespace savvy.** The working loop
  (post-#133/#134) needs the caller to bridge `fact_*` search results via
  `entity_id` and seed traversal with the `ent_*` id (entry-space-first
  resolution is correct behavior). Callers without that knowledge get
  `edges: []` and stall. A composite tool removes the requirement.

Non-goals: no librarian model changes; no re-embedding; no UI work.

## 2. Part A — Ontology Activation (recorded selection → seeded manifest)

### A.1 Seed path

On desktop-app startup (or first wiki tool use), when the entity manifest is
empty/absent AND a concrete `ontology.schema` selection is recorded: the TS
engine seeds the manifest from the selected npm schema package **pinned to the
version recorded at adoption** (`schema-software-org` → 6.2.0) via the existing
`createWiki` path, and sets ontology mode to `strict` (package manifest
authoritative, per the adoption spec's decision table). No re-onboarding
wizard; the recorded selection is the trigger.

### A.2 Idempotence and failure isolation

- Seed is once-per-DB: if a manifest is already present, startup must not
  rewrite or duplicate it.
- If the package is missing/unparseable: stay `mode: off`, emit a startup
  health warning, **never block ingest or wiki tools** (PR #78
  graceful-degradation contract extends to the ontology).

### A.3 Typed edges for new relationships

Once mode = strict, librarian-proposed relationships must use manifest edge
types. Existing edges are grandfathered untyped (back-compat read path). Edge
write path rejects types not in the manifest (strict mode), same validation
posture as the adoption spec's ingestion classification.

### A.4 Optional folder → node-type mapping (mechanism)

An optional, user-configurable mapping (config key, e.g.
`ingest.folder_type_map`: `{ "<folder-glob>": "<manifest node type>" }`) that
classifies ingested documents as additive metadata at ingest time. Never a
validation gate. A source archive with semantically organized folders (people/,
products/, decisions/, …) can thus surface typed nodes without any code change
per deployment. No mapping shipped by default; the CLI/desktop may offer the
key. Deployments configure their own instances.

## 3. Part B — Tier as a Stored Dimension

### B.1 Schema

Add `tier TEXT NULL` to `llm_wiki_entries` (schema_version +1 migration,
additive column, no backfill-in-migration). Values: `'fact'` (anchor truth),
`'wisdom'` (curated, proposal-updatable), NULL (working/unclassified — existing
entries' posture). A separate tier table was rejected: tier is intrinsic to the
entry and read on every retrieval.

### B.2 Writer contract

- Librarian synthesis: entries minted from `user_doc` chunks (tier_fact prompt
  lineage) → `'fact'`; wisdom-tier lineage → `'wisdom'`.
- Deposit-ingested entries: default from config (e.g.
  `wiki.deposit_default_tier`); **shipped default `'wisdom'`** — curated and
  still proposal-updatable, which matches the proposal-flow semantics. ('fact'
  would invoke anchor-truth freeze semantics on routinely revising agents — a
  deployment can opt in, but should not be the default.)

### B.3 Backfill (one-shot, dry-run default)

Classify existing entries only where provenance is certain: entries whose
evidence shows deposit origin → the configured deposit default; all others stay
NULL. Print a table, require `--yes` (PR #131 Part C pattern).

### B.4 Reader surface

- `wiki_search` results gain a `tier` field. NULL tier = ordinary live entry.
- `tier_fact` / `tier_wisdom` become **real, optional** `entityIds`
  namespaces (fulfilling their original #133-era intent) — explicit-only,
  never a default filter.
- Librarian prompt assembly derives labels from stored entry tier when
  present, chunk heuristics otherwise (fallback, unchanged).

## 4. Part C — Composite Context Primitive (the ergonomics surface)

### C.1 New MCP tool: `wiki_context`

Params: `query` (string, required), `depth` (uint, default 1), `max_facts`
(uint, default 5). Behavior, one call:

1. `wiki_search(query)` → top `max_facts` scored facts (default all-entries
   contract from #133 stays).
2. Collect each fact's `entity_id`; resolve and walk the entity space (both
   directions, `depth`), seeding with the entity id per the #134 resolution.
3. Return one JSON document: `{facts: [], entities: [], edges: [], provenance:
   []}` where provenance carries source doc, chunk, and score per fact.

Empty legs degrade gracefully (PR #78 contract): an entity-less fact set
returns facts with `edges: []` — never an error, never a prose fallback.

### C.2 Ergonomics contract

The caller needs zero namespace knowledge: no id-prefix distinction, no
entry-vs-entity seeding decision, no bridge mechanics. The raw tools remain for
deliberate deep work.

## 5. Validation / Acceptance Criteria

1. **AC1** On a DB with a recorded selection and no manifest, after startup:
   `wiki_get_ontology` returns a non-null manifest, mode `strict`, expected
   schema package + pinned version.
2. **AC2** Restart twice; manifest unchanged (row count stable); no duplicates.
3. **AC3** `llm_wiki_entries.tier` exists post-migration; backfill dry-run
   prints deposit-origin classifications; `wiki_search` results carry `tier`.
4. **AC4** A deposit ingests with the configured default tier (verify in DB).
5. **AC5** `wiki_context` on a known query over a populated corpus returns
   ≥1 fact AND ≥1 edge with zero namespace parameters.
6. **AC6** Fresh-session test: a caller with only `wiki_context` (no namespace
   documentation) answers a corpus question correctly from the tool output.
7. **AC7** Default `wiki_search` still returns all live entries (#133 contract
   green); explicit `tier_fact`/`tier_wisdom` namespaces work; `mode: off`
   configs still function unchanged (graceful).
8. **AC8** Full CT test suite green (cargo test --features test-utils, full
   incl tests/); all existing wiki_* tool tests pass.

## 6. Open Questions

- **Q1:** Folder→type mapping config shape (flat glob map vs nested) — decide
  at plan time.
- **Q2:** Typed manifest edges — entries only (recommended for this spec) or
  also require relationship typing at the `curated_relationships` layer?
- **Q3:** Tool name: `wiki_context` (recommended, matches wiki_* family) vs
  `context_for`.

## 7. Non-Goals

- Any specific deployment's configuration (folder maps, tier rulings,
  runbooks) — those belong to the deploying project's own specs.
- Tree-sitter / AST layer implementation (deferred by Equational Applications,
  Sep 1 2026 — irrelevant to other deployments).
- Reconciliation UI / review workflow.
- Removal of chunk.tier prompt heuristics (they remain the entry-less
  fallback).
