# Spec: Ontology Activation, Entry Tier Dimension, Composite Context Primitive

**Status:** Implemented in this PR on branch
`spec/memory-architecture-intent-implementation` (A1 ontology seed, A2
folderTypeMap mechanism, B1 V16 tier column, B2 tier filter, B3 deposit
default config + writer, B4 tier_backfill, C1 `wiki_context`). The PR
description framing ("spec-only for remote implementation") is the
spec-vs-impl pattern PR #124/#131 used; this PR both spec'd and implemented
the change in one branch, and the §3.2 writer + §2.3 edge gate + §4.1
composite tool are real, not just described.
Manual AC6 (fresh-session ergonomics check) is still deferred to PR review;
AC1/AC2 remain covered only at the unit seam (see Known gaps below).
Workspace: `.superpowers/sdd/2026-09-01-memory-architecture-intent-implementation/`.

**A2 wiring note:** `src/lib/folderTypeMap.ts`'s `resolveFolderType` and
`orderGlobs` are unit-tested and the spec describes the mechanism, but the
`ingest.folder_type_map` config key is not yet wired into the Rust ingest
path — the resolver is not called from any production code. A future change
adds the `BrainConfig` field, the call site, and the manifest-membership
validation. This PR ships the deterministic resolution + glob matching; the
config plumbing is a follow-up.

**Known gaps at time of writing** — carried openly rather than closed silently:

- **AC1 has no end-to-end test.** `seedManifestsIfAbsent` is covered at the unit
  seam; the production wiring in `setupWiki` is not. AC2 is now covered (full
  manifest content, snapshotted against a persisting store and compared after
  two restarts) — see `src/__tests__/ontologySeed.test.ts`.
- ~~**§2.3's strict-mode edge rejection does not run in CT.**~~ **Now
  implemented CT-side** (see §2.3). The investigation that led there is kept
  because it is the reason the guard lives in Rust rather than being left to
  the engine: settled against
  `core-llm-wiki` 6.2.0 source, not assumed. The engine *does* enforce it:
  `IngestionService` computes `strictEffective = opts.strict === true || mode
  === 'strict'` and passes it to `validateInlineEdges`, which throws
  `WikiStrictOntologyViolation` for an `edge_type` absent from the manifest.
  But that guard sits on `wiki.ingestDocument`, and **CT never calls it** — its
  only wrapper, `ingestDocumentByPath`, has no production call site (§3.4), and
  the app's `ingestDocument` is a Tauri invoke into the Rust pipeline. Every
  edge CT writes comes from `commit_edge_add`, a raw
  `INSERT OR IGNORE INTO llm_wiki_edges` that takes `edge_type` verbatim from
  the proposal payload; no Rust writer consults a manifest. The boundary
  therefore had to be built CT-side.
- ~~**§2.2's "one transaction for the whole seed" is not reachable at 6.2.0.**~~
  **Closed by `core-llm-wiki` 6.3.0.** The gap was real at 6.2.0:
  `WikiMemory.setOntologyManifest` opened its own `withTransactionAsync` per
  call and accepted no `tx`, and the adapter contract states nested
  transactions throw (`types.ts:25`); the repository beneath it
  (`metadataRepo.setManifest`) did take a `tx` but was not on the public
  surface. 6.3.0 adds `setOntologyManifests(entries, { ifAbsent })`, which
  writes the whole set in one transaction **it owns** — consumers pass data,
  never a transaction handle, because a consumer holds the *unwrapped* adapter
  and a transaction opened on it would bypass `withSerializedTransactions`. See
  §2.2 and the upstream spec
  `expo-llm-wiki: docs/superpowers/specs/2026-09-02-atomic-multi-entity-manifest-seed-design.md`.

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
three open questions are resolved. **Rev 3** hardens two implementation details
(deterministic glob resolution; a transactionally-committed backfill marker).
See §8 for the full changelog.

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

(`core-llm-wiki` itself is at 6.3.0 — only that engine upgrade is required
for the `setOntologyManifests(entries, { ifAbsent })` API §2.2 relies on.
The two schema packages are still 6.2.0 because the manifest content did
not change.)

### A.2 Idempotence, atomicity, and failure isolation

`seedManifests` persists one manifest per entity namespace. `core-llm-wiki`
6.3.0's `setOntologyManifests(entries, { ifAbsent })` writes a whole set in one
transaction it opens on its own serialized adapter. CT passes data only — never
a transaction handle, which would bypass the engine's serialization mutex:

- **Two atomic calls, not one.** Each call is all-or-nothing; if any entry
  fails, that whole call rolls back — never a partial set where one namespace
  is typed and another is not. It is **two** calls rather than one because the
  workspace tier's id does not exist during `setupWiki`: `getWorkspaceId()` is
  still the `tier_working::default` placeholder until `initWorkspaceId`
  resolves, so the stable tiers (`tier_fact`, `tier_wisdom`) are seeded at
  setup and the workspace tier is seeded once its id is known. Failure
  isolation is unaffected; a partial *set* within either call is unreachable.
- **Conflict-safe creation.** `ifAbsent: true` makes the check and the write
  one atomic step, closing the read-then-write TOCTOU that the previous
  `getOntology`-then-`setOntologyManifest` loop carried: a concurrent
  initializer (two windows, or startup racing a first tool call) loses the race
  harmlessly instead of erroring or double-writing. Sharp edge: `ifAbsent`
  tests for a **persisted row**, not an effective manifest, so an entity whose
  manifest so far comes only from `WikiConfig.ontology.seedManifests` is
  reported in `seeded` and its row materialized — with identical content.
- **Once-per-DB.** If a manifest is already present, startup must not rewrite
  or duplicate it. Combined with conflict-safe creation, a second seed attempt
  is a no-op rather than a rewrite.
- **Failure isolation.** If the package is missing/unparseable: stay
  `mode: off`, emit a startup health warning, **never block ingest or wiki
  tools** (PR #78 graceful-degradation contract extends to the ontology). A
  rolled-back seed leaves `mode: off`, which is a working state. No
  compensating rollback is written CT-side: the engine owns the transaction, so
  a thrown call has already left nothing behind.

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

**Where the guard lives, and why it is CT's.** This section originally assumed
the write boundary was the engine's. It is not, for CT: the engine's check runs
inside `wiki.ingestDocument`, which CT has no production caller for, while every
CT edge is written by `commit_edge_add` in Rust with no manifest lookup. The
guard is therefore implemented in the Rust commit path, resolving mode and
manifest for `ctx.entity_id` **once per commit** (the entity is fixed for the
whole proposal, so a per-item read would be pure waste) and acting only when
mode is `strict`.

The rev-2 deferral — "whether the strict-mode edge rejection surfaces as a tool
error or a skipped-with-warning proposal" — resolves to **skipped-with-warning**,
because `commit_edge_add` already does exactly that for the adjacent failure:
an edge whose endpoint will not resolve is pushed onto `ctx.dropped_edges`, the
item is marked rejected, and the commit continues. An off-manifest `edge_type` is
the same class of event — one bad item in an otherwise good proposal — and
failing the whole commit would make one hallucinated edge type discard a batch of
good facts. The drop is reported, never silent.

Three states deliberately do **not** gate, each logged: a manifest that cannot
be read (a degraded brain is not a licence to reject the librarian's whole
output — PR #78), a non-`strict` mode, and `strict` with an empty `edge_types`
list. The last is the one judgement call: a gate needs a vocabulary to be a
gate, and `strict` with zero declared edge types is far more likely a
half-finished seed than a deliberate "no edges permitted" policy, where
silently dropping every edge of every proposal would be severe and very hard to
diagnose. It is the most permissive reading of an ambiguous state, and it is
pinned by a test so that flipping it is a conscious decision.

Edge-type membership is compared **case-insensitively**, matching the engine's
own `resolveEdgeDefinitions`; a guard stricter than the producer would reject
types the librarian was told were legal.

### A.4 Optional folder → node-type mapping (mechanism)

An optional, user-configurable mapping — config key `ingest.folder_type_map`,
a **flat glob→type map** (`{ "<folder-glob>": "<manifest node type>" }`) — that
classifies ingested documents as additive metadata at ingest time. Flat over
nested: the
value is a single scalar, so nesting would buy only grouping at the cost of a
deeper schema and a merge rule.

**Resolution order is parser-independent.** Map iteration order must never be
load-bearing: JSON object key order is not guaranteed, and the config round-trips
through `preserved_keys` (`config/mod.rs:70`) as raw `serde_json::Value`, whose
ordering depends on build features. The implementation therefore sorts the glob
set into a total order before matching, and the *first* glob in that order that
matches wins:

1. Descending literal specificity — count of non-wildcard path segments, then
   total literal (non-metacharacter) length.
2. Ascending lexicographic on the glob string, as the final tie-break.

Two globs can then never be tied, so the selected type is identical on every
platform and parser. A test asserts the same map given in two different key
orders produces identical classifications.

Never a validation gate; an unmatched document
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

- Deposit-ingested entries: default from config `wiki.deposit_default_tier`,
  stamped at write time in the commit path (`commit_fact_add`). Deposit origin
  is the only classification a writer can make with certainty, so it is the only
  one it makes: an entry is deposit-origin when any evidence chunk resolves to a
  document under the agent deposit directory.
- Everything else stays NULL — the working/unclassified posture, where the
  librarian falls back to its chunk heuristics. Deriving `'fact'` from
  `user_doc` chunk lineage is deliberately **not** implemented: `chunk.tier`
  distinguishes `user_doc` from `wiki`, which is a *source-folder* distinction,
  not the revisability distinction tier encodes. Stamping every `user_doc`-derived
  entry `'fact'` would freeze the bulk of the corpus under "ANCHOR TRUTH — do not
  propose modifications" on the strength of where a file happened to sit.

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

**Authoritative provenance predicate.** The backfill classifies a row as
deposit-origin when **any** of its evidence chunks resolves to a document
under the agent-deposit directory. The single, load-bearing rule lives in
`tools/src/tier_backfill.rs` (`eligible_rows_predicate` + the two
`deposit_path_predicate` forms) and the matching `safe_path::is_deposit_path`
helper (`src-tauri/src/vault/safe_path.rs`); the writer and the tool must
never drift, and any change to one is a change to the other. The predicate
matches both path shapes the database actually holds (absolute and
vault-relative), normalizes Windows separators, and uses a trailing `/` so
`agents-but-not-really/` is a sibling directory, not a deposit. Missing or
ambiguous evidence (a `chunk_id` lookup that returns no row, an empty
`content_hash`, a `source_ref` that is neither a recognised legacy path nor
a valid JSON blob) leaves the row out of the plan; the row stays NULL and
the run is not failed by it.

**Deposit provenance is tested in both path shapes.** `documents.path` is
written by the ingest walker, which canonicalizes to an **absolute** path, while
the legacy `source_ref` producer and every fixture store a **vault-relative**
path. A predicate anchored to the relative prefix alone therefore matches
nothing on a real brain while passing every relative-path unit test — so the
anchored form is paired with a `'%/' || prefix` form, and the Rust writer shares
the same two-shape test via `safe_path::is_deposit_path`. The trailing separator
is load-bearing: it makes the test a path-segment test rather than a string
prefix, so `agents-but-not-really/` is not a deposit.

"One-shot" is enforced, not assumed:

- **The marker parameterizes the run; it does not gate it.** On successful
  apply, record `tier_backfill_v1` in **`llm_wiki_meta`** (`key TEXT PRIMARY
  KEY, value TEXT NOT NULL`, `db/okf_ddl.rs:124`) — **in the same SQLite
  transaction as the tier UPDATEs**. The value is JSON, not a boolean:

  ```json
  {"version": 1, "first_applied_at": <unix>, "last_applied_at": <unix>,
   "runs": <n>, "deposit_default_used": "wisdom",
   "rows_classified": <cumulative n>, "schema_version": <n>}
  ```

  A later apply does **not** exit on finding the marker. It runs, and takes
  `deposit_default_used` — the cohort's original value — in place of current
  config for any row it newly classifies. Combined with the NULL-only scope
  below, this is idempotent by construction: already-classified rows are out of
  scope, and a row whose deposit provenance only became visible after run 1
  joins the cohort at the cohort's tier rather than splitting it.

  **Marker update on rerun.** A rerun that classifies ≥1 row rewrites the marker
  in the same transaction as those UPDATEs, so the ledger stays accurate rather
  than frozen at run 1. Fields split into two classes:

  | Field | On rerun |
  |---|---|
  | `deposit_default_used` | **Write-once — never rewritten.** |
  | `version`, `first_applied_at` | Write-once. |
  | `rows_classified` | Accumulates (`old + newly classified`). |
  | `runs` | Increments. |
  | `last_applied_at`, `schema_version` | Overwritten with the current run's. |

  `deposit_default_used` being write-once is load-bearing, not bookkeeping: if a
  rerun refreshed it from current config, the cohort value would follow config
  drift and the marker would decay into exactly the current-config behavior it
  exists to prevent. `first_applied_at` is kept because a single overwritten
  `applied_at` cannot answer when the cohort was established, only when it was
  last touched — the ledger needs both.

  A rerun that classifies **zero** rows **and finds the existing marker
  present** writes nothing at all: no marker rewrite, no transaction,
  consistent with the dry-run-default posture that a run which changes no
  data leaves no trace. `last_applied_at` therefore means "last run that
  wrote rows", not "last run attempted".

  If the marker is absent (an operator deleted it), the rerun writes a fresh
  marker regardless of `changed` — the recovery direction must always leave a
  ledger behind. The new marker's `deposit_default_used` is the current
  config default (the prior cohort is gone, so a write-once copy of it is
  impossible), `first_applied_at` is the run's wall clock, and
  `rows_classified` is the count of rows *that run* newly classified
  (zero if everything was already tier != NULL — the floor is then 0, not
  a total). The count is then a floor, not a total. This is accepted:
  reconstructing the true total would require re-deriving provenance for
  every non-NULL row, and the field is a ledger for operators, never an
  input to any decision.

  **This is what removes the dangerous crash direction.** A gate that refuses on
  sight of a marker has an unrecoverable failure: a marker present without its
  UPDATEs permanently suppresses a backfill that never ran, and the NULL-only
  scope cannot repair it because those rows are still NULL and now unreachable.
  With a parameterizing marker, neither direction is harmful — a **lost** marker
  degrades to current-config behavior (and cannot retier anything, per NULL-only),
  and a **spurious** marker merely supplies a default to whatever rows are
  eligible. There is no state from which the operator cannot recover.

  Same-transaction commit is retained anyway, as defense in depth and because it
  keeps `rows_classified` truthful. It also forces the store choice: `llm_wiki_meta`
  is in the same database as `llm_wiki_entries`, so one transaction covers both.
  **A config-file marker is rejected** — two stores with no shared commit cannot
  be made atomic.

  `version` is what a future classifier change reads: a `v1` marker seen by a
  `v2` classifier is reported and left alone rather than silently reclassifying
  under new rules.
- **NULL-only updates.** Even absent the marker, the UPDATE is scoped to
  `WHERE tier IS NULL`. An entry classified by a writer, by a human, or by an
  earlier run is never reclassified.
- **Config drift is inert, in both directions.** These two together mean
  changing `wiki.deposit_default_tier` after a backfill cannot retier
  already-backfilled rows (NULL-only excludes them) *and* cannot split the
  cohort on a later run (the marker supplies the original value). The config
  governs only new deposits from that point on. This is the intended semantics:
  it is a writer default, not a retroactive policy.

Tests: apply without `--yes` mutates nothing and exits non-zero; apply, then
flip `wiki.deposit_default_tier`, then re-apply → every previously written tier
is unchanged; a row whose deposit provenance appears only after run 1 is
classified on re-apply at `deposit_default_used`, **not** at the flipped config
value; that same rerun leaves `deposit_default_used` and `first_applied_at`
unchanged while `rows_classified` accumulates and `runs` increments; a rerun
classifying zero rows leaves the marker byte-identical; a transaction aborted
after the UPDATEs but before commit leaves both the tiers NULL and the marker
absent; and — the recovery property — deleting the marker and re-applying is a
no-op on all previously classified rows.

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
  present, chunk heuristics otherwise (fallback, unchanged). **Status: the
  stored `llm_wiki_entries.tier` column is written by the deposit path and
  the backfill, but the librarian prompt assembly does not yet read it.**
  `assemble_librarian_context` (`src-tauri/src/librarian/mod.rs:26-43`)
  still labels input from `chunk.entity_id` and `documents.tier` —
  unchanged from the pre-V16 behaviour — so a stored `'fact'` does not yet
  surface as "ANCHOR TRUTH" in the prompt. The bridge is a separate
  follow-up: prompt semantics for the curated `'fact'`/`'wisdom'` labels
  ride a later PR that wires `entries.tier` into the synthesis path.
  Until then, the column is storage-truth only and the prompt keeps the
  chunk-heuristics fallback unchanged.

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
   `entityIds` **filtering and selection** behavior is byte-identical to
   v1.40.1 (no namespace was added or migrated); `mode: off` configs still
   function unchanged (graceful). The full response is *not* byte-identical
   to v1.40.1 — B.4 adds a `tier` field to every `wiki_search` result, so a
   raw JSON diff picks up the additive field. "Byte-identical" here means
   the **set of returned entries and the per-row `entityIds` matching**
   are unchanged, which is the only behaviour the #133 contract asserts;
   the new `tier` field is exercised by a separate assertion in AC3.
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

Deferred to plan time (implementation detail, not design risk): whether the
strict-mode edge rejection surfaces as a tool error or a skipped-with-warning
proposal. The `tier_backfill_v1` marker store was deferred in rev 2 and is
settled in rev 3 (§3.3): `llm_wiki_meta`, committed in the data transaction.

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

**Rev 4 (2026-09-02)** — implementation review response. Changes made in code,
recorded here so the spec matches what shipped:

| # | Change | Source |
|---|---|---|
| 17 | §3.2 deposit default given an actual writer (`commit_fact_add`); the `user_doc`→`'fact'` lineage rule is withdrawn with rationale | Review — the config key had no writer at all, so the setting was inert until the offline backfill ran |
| 18 | §3.3 provenance predicate matches absolute **and** vault-relative document paths | Review — production `documents.path` is absolute, so the shipped anchored-relative predicate classified zero rows on every real brain while its relative fixtures passed |
| 19 | §4 `wiki_context` implemented: dispatcher arm, MCP registration, composite walker with the node cap applied across all seeds; AC5/AC8 covered | Review — Part C had no implementation in any file |
| 20 | §2.1 seed no longer runs against the placeholder workspace id | Review — `setupWiki` resolves before React mounts, so `getWorkspaceId()` was still `tier_working::default`; the real workspace tier went unseeded and a junk entity was written |
| 21 | Tier vocabulary centralized (`schema::VALID_TIERS` / `is_valid_tier`) and read by both write boundaries | Review — the set was hand-coded in three places with no shared validator |
| 22 | `wiki.deposit_default_tier` now survives the lenient config path | Review — `wiki` was in `known_keys` (so excluded from `preserved_keys`) but had no extraction branch, silently losing the setting |
| 23 | Per-field `#[serde(default)]` on `BrainConfig`'s typed blocks reverted | Review — it made strict `load()` succeed on an incomplete config, defeating the gate that routes such a config to `load_lenient`; a regression guard now pins the asymmetry |
| 24 | `folder_type_map` glob translation rewritten as a single pass | Review — `?` was left live as a regex quantifier, and staging `**` through a space placeholder corrupted any glob containing a space |

| 25 | §2.3 strict-mode edge gate implemented in `commit_edge_add`, resolved once per commit, skip-with-warning via the existing `dropped_edges` path | Settled against `core-llm-wiki` 6.2.0 source: the engine enforces this only inside `ingestDocument`, which CT never calls |
| 26 | `WikiManifest` reparsed to the shape the engine actually persists | **`wiki_get_ontology` could not read a real manifest at all.** The engine writes `edge_types` as objects (`{type, source_type, target_type, description}`); CT typed them `Vec<String>`, so `serde_json::from_str` failed on every seeded brain and the tool returned an error a caller cannot distinguish from "no ontology". Bare strings are still accepted so a legacy row degrades instead of failing |
| 27 | AC2 covered: full manifest content snapshotted against a persisting store, compared equal after two restarts, with a companion test proving the snapshot detects a same-count content swap | Review — AC2 had no coverage at all |

Two review findings were **not** adopted, on the evidence:

- **Rekeying `tier_weight` off the stored `tier` column.** §3.4 keeps
  `tier_fact`/`tier_wisdom` as "live ranking-weight keys", and AC7 requires
  `entityIds` behavior byte-identical to v1.40.1. Rekeying ranking would change
  scores for existing rows — the one thing this additive spec promised not to do.
- **Applying the `tier` filter before `wiki_search`'s empty-`entityIds` early
  return.** `Some(&[])` matches nothing by contract, so the result is already
  correct; only the diagnostic would differ, and #133's explicit-empty contract
  is worth more than the message.

**Rev 5 (2026-09-02)** — upstream dependency landed. One change:

| # | Change | Source |
|---|---|---|
| 28 | §2.2's atomicity gap closed: `core-llm-wiki` 6.3.0 ships `setOntologyManifests(entries, { ifAbsent })`, so the read-then-write loop and its compensating rollback are deleted. Restated as **two** atomic calls, since the workspace tier's id does not exist during `setupWiki` | The gap was carried openly in rev 3; the fix was specced upstream (`expo-llm-wiki`, `2026-09-02-atomic-multi-entity-manifest-seed-design.md`) and released as 6.3.0 |

**Rev 3 (2026-09-01)** — implementation-hardening review (Kurt). Three changes,
all closing crash/platform-determinism windows rather than altering design:

| # | Change | Source |
|---|---|---|
| 13 | §2.4 glob resolution given a total order (specificity, then lexicographic) so matching never depends on JSON key order or `preserved_keys` round-tripping | Review — rev 2's "first match on tie" leaned on parser-dependent iteration order |
| 14 | §3.3 marker store settled on `llm_wiki_meta`, committed in the same transaction as the tier UPDATEs; config-file marker rejected as non-atomic; Q closed in §6 | Review flagged the transactional requirement; the same-DB KV table makes it resolvable now rather than at plan time |
| 15 | §3.3 marker changed from a boolean **gate** to a JSON **parameter** (`deposit_default_used` et al); reruns are permitted and pin the cohort's original tier | Review asked whether the crash window could be made less dangerous. It can be made harmless: gating is what created the unrecoverable direction, so the gate was removed rather than further guarded |
| 16 | §3.3 marker rewrite-on-rerun specified: `rows_classified` accumulates and `runs`/`last_applied_at` advance, while `deposit_default_used`/`first_applied_at` are write-once; zero-row reruns write nothing | Review — rev 3 left rerun marker handling undefined. Aggregation adopted as proposed; the write-once split is the correction that keeps the cohort value from decaying under config drift |

**Rev 2 (2026-09-01)** — review response. Changes:

| # | Change | Source |
|---|---|---|
| 1 | §3.4 rewritten: tier is a `wiki_search` filter, not an `entityIds` namespace | Own review — rev 1 required the writer migration PR #135 rejected; `tier_fact`/`tier_wisdom` have no production writer and `entityIdForPath` tests the v1 prefix |
| 2 | §2.2 seed made transactional + conflict-safe with rollback | Review (atomic multi-entity seeding). Superseded by change 17: the rollback is gone, the engine owns the transaction |
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
