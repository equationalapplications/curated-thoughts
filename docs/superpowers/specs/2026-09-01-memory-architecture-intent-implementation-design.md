# Spec: Ontology Activation, Entry Tier Dimension, Composite Context Primitive

Baseline: v1.40.1 (`d31c208`, main). Scope: **Curated Thoughts codebase only** —
general-purpose features for any deployment using an immutable source archive +
curated-wisdom memory workflow. The motivating deployment (Equational Applications'
agent memory) has its own deployment spec in that project's private vault and is
referenced here only as context. Sibling lineage:
2026-08-31-wiki-graph-reanchor-entry-embeddings-design.md (PR #131) and
2026-09-01-wiki-reader-contract-and-forget-outbox-design.md (PR #135, v1.40.1).
Docs ride the implementing PR (Kurt directive, Aug 31 2026).

**Rev 2 (2026-09-01)** — revised after review. The load-bearing change is §3.4:
rev 1 proposed making `tier_fact`/`tier_wisdom` real `entityIds` namespaces,
which silently required the writer migration PR #135's spec explicitly rejected.
Tier is now a first-class filter, fully decoupled from `entity_id`. Remaining
revisions close write-boundary, idempotence, and acceptance-criteria gaps; all
three open questions are resolved. See §8 for the full changelog.

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
  not queryable as such.
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

### A.2 Idempotence, atomicity, and failure isolation

`seedManifests` persists one manifest per entity namespace. `core-llm-wiki`'s
`OntologyService.setManifest(entityId, data, tx)` takes an `SQLiteAdapter`
transaction handle but does **not** wrap a multi-entity seed in one itself —
atomicity is the caller's responsibility. This spec makes CT that caller:

- **One transaction for the whole seed.** All manifests written under a single
  `tx`. If any write fails, the entire seed rolls back — never a partial set
  where one namespace is typed and another is not.
- **Conflict-safe creation.** Each write is an idempotent
  create-if-absent against the `entity_id` uniqueness constraint, so a
  concurrent initializer (two windows, or startup racing a first tool call)
  loses the race harmlessly instead of erroring.
- **Once-per-DB.** If a manifest is already present, startup must not rewrite
  or duplicate it. Combined with conflict-safe creation, a second seed attempt
  is a no-op rather than a rewrite.
- **Failure isolation.** If the package is missing/unparseable: stay
  `mode: off`, emit a startup health warning, **never block ingest or wiki
  tools** (PR #78 graceful-degradation contract extends to the ontology). A
  rolled-back seed leaves `mode: off`, which is a working state.

Tests: concurrent initialization converges on one manifest set; an injected
mid-seed failure leaves zero manifests and `mode: off`, not a partial set.

### A.3 Typed edges for new relationships — the write boundary

Once mode = strict, librarian-proposed relationships must use manifest edge
types. Precisely:

- **In scope: `llm_wiki_edges`.** The semantic knowledge graph. Writes whose
  `edge_type` is absent from the manifest are **rejected** in strict mode, the
  same validation posture as the adoption spec's ingestion classification.
- **Out of scope: `curated_relationships`.** Review raised this as an open
  boundary; it is not one. `curated_relationships`
  (`from_id, to_id, rel_type, symbol, entity_id`) is the **AST symbol-linker
  graph**, written mechanically by `indexer/linker.rs` with structural
  `rel_type`s (`CALLS`, `IMPORTS`) derived from code, not proposed by the
  librarian. A manifest of domain node/edge types has no vocabulary for it, and
  gating the linker on one would break code indexing on every strict brain.
- **Reads stay untyped-tolerant.** Existing edges are grandfathered: the read
  path never filters on manifest membership, so pre-strict rows remain visible
  and traversable. Validation is a write-time gate only.

Tests: a manifest-defined `edge_type` writes successfully in strict mode; an
unknown type is rejected with a diagnostic naming the manifest; a grandfathered
untyped row still reads and traverses; `curated_relationships` writes are
unaffected by mode.

### A.4 Optional folder → node-type mapping (mechanism)

An optional, user-configurable mapping — config key `ingest.folder_type_map`,
a **flat glob→type map** (`{ "<folder-glob>": "<manifest node type>" }`;
resolved most-specific-glob-wins, first match on tie) — that classifies
ingested documents as additive metadata at ingest time. Flat over nested: the
value is a single scalar, so nesting would buy only grouping at the cost of a
deeper schema and a merge rule. Never a validation gate; an unmatched document
ingests unclassified, and a glob naming a type absent from the manifest logs a
warning and is skipped rather than failing ingest. A source archive with
semantically organized folders (people/, products/, decisions/, …) can thus
surface typed nodes without any code change per deployment. No mapping shipped
by default.

## 3. Part B — Tier as a Stored Dimension

### B.1 Schema

Add `tier TEXT NULL` to `llm_wiki_entries` (schema_version +1 migration,
additive column, no backfill-in-migration), with a **`CHECK (tier IN ('fact',
'wisdom') OR tier IS NULL)`** constraint in the same migration. Values:
`'fact'` (anchor truth), `'wisdom'` (curated, proposal-updatable), NULL
(working/unclassified — existing entries' posture).

The CHECK is the invariant's floor: a bare `TEXT NULL` accepts any string, and
an entry with an out-of-vocabulary tier would carry no prompt semantics and
match no filter. Write boundaries validate against the same three-value set so
callers get a diagnostic rather than a constraint violation, but the database
is the authority. The constraint admits every existing row unchanged (all NULL
pre-backfill), so the migration needs no data pass.

A separate tier table was rejected: tier is intrinsic to the entry and read on
every retrieval.

### B.2 Writer contract

- Librarian synthesis: entries minted from `user_doc` chunks (tier_fact prompt
  lineage) → `'fact'`; wisdom-tier lineage → `'wisdom'`.
- Deposit-ingested entries: default from config `wiki.deposit_default_tier`.

**Shipped default: `'wisdom'`. This is decided, not open** (rev 1 left it
ambiguous — asserting `'wisdom'` here while listing it as blocking elsewhere).
Rationale: deposits are agent-written notes under active revision, and
`'fact'` invokes the librarian's "ANCHOR TRUTH — do not propose modifications"
framing, which would freeze exactly the content agents are expected to keep
correcting. A deployment that treats deposits as immutable record can set
`'fact'` explicitly.

Note the path-shape argument does *not* override this: deposits land at
`immutable-source-files/agents/` (`safe_path.rs` `AGENTS_DEPOSIT_DIR`), inside
the immutable prefix, but that placement is a *write-permission* boundary
(`NOTE_WRITABLE_SUBDIRS` grants agents write access there), not a truth claim.
Tier follows revisability, not directory.

AC4 asserts this same value.

### B.3 Backfill (one-shot, dry-run default, durably idempotent)

Classify existing entries only where provenance is certain: entries whose
evidence shows deposit origin → the configured deposit default; all others stay
NULL. Print a table, require `--yes` (PR #131 Part C pattern).

"One-shot" is enforced, not assumed:

- **Completion marker.** On successful apply, record a backfill marker
  (`tier_backfill_v1`) in the brain's migration/marker state. A subsequent
  apply reads the marker and exits as a no-op, reporting that it already ran.
- **NULL-only updates.** Even absent the marker, the UPDATE is scoped to
  `WHERE tier IS NULL`. An entry classified by a writer, by a human, or by an
  earlier run is never reclassified.
- **Config drift is inert.** These two together mean changing
  `wiki.deposit_default_tier` after a backfill cannot retier already-backfilled
  rows — it governs only new deposits from that point on. This is the intended
  semantics: the config is a writer default, not a retroactive policy.

Tests: apply without `--yes` mutates nothing and exits non-zero; apply,
then flip `wiki.deposit_default_tier`, then re-apply → second run is a no-op
and every previously written tier is unchanged.

### B.4 Reader surface — tier is a filter, not a namespace

**This section is the rev-2 correction.** Rev 1 proposed that
`tier_fact`/`tier_wisdom` "become real, optional `entityIds` namespaces,
fulfilling their original #133-era intent." That cannot ship as written:

- `tier_fact`/`tier_wisdom` **have no production writer.** In Rust they appear
  only in tests and in `wiki_graph.rs:72-77`'s ranking weights. `entityIdForPath`
  (`src/lib/wikiTiers.ts`) does map `documents/` → `tier_fact` and `wiki/` →
  `tier_wisdom`, but it still tests the **v1** prefix: the v2 layout renamed
  that folder to `immutable-source-files/` (`safe_path.rs:33`, `migrate_vault`),
  so on any v2 vault the branch is unreachable. Its only caller,
  `ingestDocumentByPath`, has no production call site — the live ingest path is
  Rust-side and writes other `entity_id`s.
- Making those namespaces real therefore requires **migrating the writer to
  tier namespaces** — the alternative PR #135's spec explicitly rejected:
  *"requires migrating 140 rows and repointing 88 edges' expectations, touching
  PR #78's stale-SQL history. A reader-only change cannot corrupt data; this
  one can."* Rev 1 would have smuggled that rejected migration in as a
  side effect of a tier column.

The corrected design **decouples tier from `entity_id` entirely**:

- `wiki_search` results gain a `tier` field (`'fact' | 'wisdom' | null`).
  NULL = ordinary live entry.
- `wiki_search` gains an **optional `tier` filter parameter** accepting
  `'fact'`, `'wisdom'`, or omitted. Omitted is the default and preserves #133's
  all-live-entries contract exactly. This is the supported way to ask for the
  curated layer.
- **`entityIds` semantics are untouched.** No new namespace is defined,
  populated, or migrated to. `tier_fact`/`tier_wisdom` keep their current
  status: live ranking-weight keys, honored per-row wherever such rows exist,
  never written by this spec.
- Librarian prompt assembly derives labels from stored entry tier when
  present, chunk heuristics otherwise (fallback, unchanged).

This is strictly better than the namespace overload even setting the migration
aside: tier and partition are orthogonal, so a brain can have both a real
`entity_id` partition and a tier without the two contending for one column.

Follow-up (not this spec): repairing `entityIdForPath`'s v1/v2 prefix drift and
deciding whether the router gets a production caller at all. Filed separately
because it is a writer-side change with the data-migration risk PR #135
identified, and this spec is deliberately additive.

## 4. Part C — Composite Context Primitive (the ergonomics surface)

### C.1 New MCP tool: `wiki_context`

Name decided: **`wiki_context`** (matches the `wiki_*` family; `context_for`
dropped). Rev 1 listed this as open while C.1 and the acceptance criteria
already used it — the ambiguity risked incompatible registrations and tests.

Params: `query` (string, required), `depth` (uint, default 1), `max_facts`
(uint, default 5), `tier` (optional, `'fact' | 'wisdom'`, per §3.4).
Behavior, one call:

1. `wiki_search(query)` → top `max_facts` scored facts (default all-entries
   contract from #133 stays; `tier` forwarded if supplied).
2. Collect each fact's `entity_id`; resolve and walk the entity space (both
   directions, `depth`), seeding with the entity id per the #134 resolution.
3. Return one JSON document: `{facts: [], entities: [], edges: [],
   provenance: [], truncated: bool}` where provenance carries source doc,
   chunk, and score per fact.

**Traversal limits are the existing ones, reused — not new ones.**
`wiki_context` is a composition over the #134 traversal path and inherits its
contract verbatim: `clamp_max_depth` (1..3, `wiki_graph.rs:82`),
`MAX_TRAVERSAL_NODES = 50` (`wiki_graph.rs:11`), the BFS visited set, and
edge-key deduplication. A `depth` above 3 is clamped, not rejected. Because one
`wiki_context` call fans out from up to `max_facts` seeds, the node cap is
applied across the **whole composite walk**, not per seed — otherwise the
effective ceiling would be `max_facts × 50`. `truncated` is true when the cap or
the depth clamp cut the walk short, so a caller can tell a complete
neighborhood from a partial one.

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
2. **AC2** Snapshot the canonical manifest contents (node ids, node types, edge
   types, and version — not merely row count) after first seed; restart twice;
   the full snapshot compares equal after each restart. Row-count stability
   alone is insufficient: ids or types can change while the count holds.
3. **AC3** `llm_wiki_entries.tier` exists post-migration and its CHECK rejects
   an out-of-vocabulary value; backfill dry-run prints deposit-origin
   classifications and mutates nothing; `wiki_search` results carry `tier`.
4. **AC4** A deposit ingests with tier `'wisdom'` (the §3.2 shipped default),
   verified in DB; setting `wiki.deposit_default_tier: 'fact'` makes the next
   deposit `'fact'`.
5. **AC5** `wiki_context` over a **linked fixture** — at least one fact whose
   `entity_id` resolves to an entity carrying at least one live relationship —
   returns ≥1 fact AND ≥1 edge with zero namespace parameters. The fixture must
   be linked by construction; "a populated corpus" does not guarantee a
   resolvable edge, and an unlinked corpus legitimately returns `edges: []`
   under the §4.1 graceful-degradation contract.
6. **AC6** Fresh-session test: a caller with only `wiki_context` (no namespace
   documentation) answers a corpus question correctly from the tool output.
7. **AC7** Default `wiki_search` still returns all live entries (#133 contract
   green); the explicit `tier` filter returns exactly the matching entries;
   `entityIds` behavior is byte-identical to v1.40.1 (no namespace was added or
   migrated); `mode: off` configs still function unchanged (graceful).
8. **AC8** `wiki_context` with `depth: 9` clamps to 3; a walk exceeding
   `MAX_TRAVERSAL_NODES` across all seeds returns `truncated: true` with at
   most 50 nodes.
9. **AC9** Full CT test suite green (cargo test --features test-utils, full
   incl tests/); all existing wiki_* tool tests pass.

## 6. Open Questions

None blocking. Rev 1's Q1–Q3 are resolved in place: folder→type map shape is
flat glob→type (§2.4); manifest edge typing covers `llm_wiki_edges` only, with
`curated_relationships` out of scope on the evidence in §2.3; tool name is
`wiki_context` (§4.1). The deposit default is likewise settled at `'wisdom'`
(§3.2).

Deferred to plan time (implementation detail, not design risk): the exact
marker store for `tier_backfill_v1`, and whether the strict-mode edge rejection
surfaces as a tool error or a skipped-with-warning proposal.

## 7. Non-Goals

- Any specific deployment's configuration (folder maps, tier rulings,
  runbooks) — those belong to the deploying project's own specs.
- Tree-sitter / AST layer implementation (deferred by Equational Applications,
  Sep 1 2026 — irrelevant to other deployments).
- Reconciliation UI / review workflow.
- Removal of chunk.tier prompt heuristics (they remain the entry-less
  fallback).
- **Any writer-side migration to tier `entity_id` namespaces**, including
  repairing `entityIdForPath`'s v1/v2 prefix drift. Explicitly out of scope per
  §3.4; PR #135's spec rejected this class of change as the only one that can
  corrupt data, and nothing here reopens it.

## 8. Revision history

**Rev 2 (2026-09-01)** — review response. Changes:

| # | Change | Source |
|---|---|---|
| 1 | §3.4 rewritten: tier is a `wiki_search` filter, not an `entityIds` namespace | Own review — rev 1 required the writer migration PR #135 rejected; `tier_fact`/`tier_wisdom` have no production writer and `entityIdForPath` tests the v1 prefix |
| 2 | §2.2 seed made transactional + conflict-safe with rollback | Review (atomic multi-entity seeding) |
| 3 | §2.3 edge write boundary scoped to `llm_wiki_edges`; `curated_relationships` excluded with rationale | Review raised the boundary; scope decision is own analysis — it is the AST linker graph, not librarian output |
| 4 | §3.1 CHECK constraint on `tier` | Review (tier value invariant) |
| 5 | §3.2 deposit default resolved to `'wisdom'`, contradiction removed | Review (inconsistent shipped default) |
| 6 | §3.3 backfill given completion marker + NULL-only scope + config-drift semantics | Review (durable idempotence) |
| 7 | §4.1 traversal limits inherited explicitly; node cap defined across the composite walk; `truncated` added | Review; the cross-seed cap clarification is own analysis |
| 8 | §4.1 tool name fixed to `wiki_context`; Q3 closed | Review (name ambiguity) |
| 9 | AC2 compares full manifest content, not row count | Review |
| 10 | AC5 requires a linked fixture | Review |
| 11 | AC8 added for clamp/truncation; AC4 asserts the decided default | Follows from 5 and 7 |
| 12 | §2.4 folder-map shape decided (flat); Q1 closed | Rev 1 deferral |

Not adopted from review: the request to define tier-namespace population across
all writers (finding on rev-1 §3.4). The correct resolution is not to populate
those namespaces at all — see §3.4 and §7.
