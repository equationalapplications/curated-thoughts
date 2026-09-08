# Spec: Wiki Edge Integrity Wave (PRs 1–3)

**Status:** Implemented (PR 1)
**Date:** 2026-09-08
**Baseline:** `main` @ `aec3d2e` (v2.6.0)
**Issues:** #189 (edge writer emits non-canonical case + same-name dupes),
#191 (`llm_wiki_edges.created_at` mixes seconds and milliseconds),
#190 (activate strict manifests on `ent_*` + entity-scoped MCP traversal).

PR 1 (this design's §2) is implemented on branch `spec/issue189-edge-writer-correctness`. PR 2 (manifest activation + sweep arming, §3) and PR 3 (entity-scoped traversal, §4) are still open.
**Scope:** Curated Thoughts codebase only. No upstream (`core-llm-wiki` /
`expo-llm-wiki`) changes.

Three changes to the semantic-graph write path and its enforcement surface,
sequenced so each is verifiable before the next lands.

| PR | Title | Issues | Size | Depends on |
| --- | --- | --- | --- | --- |
| 1 | Edge writer correctness: canonical type, same-name guard, ms `created_at` + V19 | #189, #191 | ~350 lines | none |
| 2 | Manifest activation on `ent_*` + sweep armed post-librarian-run | #190 (part 1) | ~200 lines | PR 1 merged |
| 3 | Entity-scoped traversal: diagnose, then fix | #190 (part 2) | unknown until §4.1 runs | PR 2 merged |

**Why not one PR.** PR 2 arms a sweep that deletes rows. Arming it before
PR 1 stops producing the noise means the sweep purges live librarian output on
every run, and any resulting data loss is indistinguishable from a sweep bug.
PR 1 also rewrites production timestamps (V19); mixing a data migration with
an enforcement activation makes a bad-outcome bisect nearly impossible. PR 3
is gated on PR 2 because the reproduction in §4.1 must run against a brain
that actually has `ent_*` manifest rows — otherwise it measures the wrong
world.

---

## §1 — Shared context

Facts that are easy to get wrong and expensive to get wrong. Every PR reads
this section before touching code.

### 1.1 Where edges are written

`llm_wiki_edges` has many insert sites. The one the Active Librarian drives —
and the only one implicated in #189 — is:

- `commit_edge_add`, `src-tauri/src/db/commit.rs:1575`

The others, all of which PR 1 must audit for the `created_at` unit (§2.4) but
none of which change behavior otherwise:

- `src-tauri/src/lib.rs:5035`, `:5041`, `:5233`
- `src-tauri/src/wiki_graph.rs:758`
- `src-tauri/src/db/connections.rs:249`, `:342`, `:375`
- `src-tauri/src/db/bundle_io.rs:196`, `:259`
- `src-tauri/src/db/commit.rs:2239`, `:3478`, `:3708` (test fixtures)
- `src-tauri/src/db/wiki_forget.rs:107`
- `src-tauri/src/db/edge_purge.rs:392`, `:444` (test fixtures)
- `src-tauri/src/db/wisdom.rs:658`

The table's uniqueness constraint is
`UNIQUE(entity_id, source_id, target_id, edge_type)`. Case-variant types are
therefore *distinct rows* under it — which is why `dependson` and `dependsOn`
coexist rather than colliding, and why the canonicalization fix in §2.2 both
removes the noise and turns future case variants into `INSERT OR IGNORE`
no-ops.

### 1.2 The strict gate as it exists today

`resolve_strict_edge_vocabulary` (`commit.rs:154`) returns
`Option<HashSet<String>>` of **lowercased** manifest edge-type names, or
`None` when writes are not gated. `None` is returned in three deliberate
cases, documented at the function: mode is not `strict`; the ontology could
not be read (PR #78 graceful degradation); strict mode declares zero edge
types.

Manifests are seeded against **partitions** (`tier_fact`, `tier_wisdom`,
`tier_working::*`), not curated ids, so the resolver looks up
`[entity_id, "tier_fact"]` in order and the partition fallback is the typical
production path for an `ent_*` proposal.

The gate at `commit.rs:1617` is:

```rust
if !vocabulary.contains(&edge_type.trim().to_lowercase()) { /* drop */ }
```

Read that carefully: it lowercases the *candidate* and compares against a
lowercased *vocabulary*, then `commit.rs:1639` inserts `edge_type`
**verbatim**. `dependson` passes the gate and is written as `dependson`. This
is the whole of #189's case defect — the gate is not missing, it is
case-insensitive by construction and discards the canonical spelling it
matched against.

The same `resolve_strict_edge_vocabulary` backs two other call sites, and both
inherit whatever this function returns:

- `purge_off_manifest_edges_in_tx`, `edge_purge.rs:242`
- `wiki_traverse_graph`, `wiki_graph.rs:648`

### 1.3 The seconds/milliseconds duality

`SEC_VS_MS_THRESHOLD` = `1_000_000_000_000` (twelve zeros, 2001-09-09 in ms).
Any value at or above it is milliseconds; any positive value below it is
seconds. The constant and its invariants are pinned by
`src-tauri/tests/timestamp_units.rs`.

`commit.rs:1639` passes `ctx.now_secs` into `llm_wiki_edges.created_at` while
`CommitContext` also carries a millisecond clock (`now_timestamps()` returns
both; `ms_now()` at `commit.rs:437`). That single argument is the root of
#191.

### 1.4 Migration mechanics

Migrations live in `src-tauri/src/db/schema.rs` as `MIGRATION_V<n>` constants
and are applied in `src-tauri/src/db/connection.rs` under
`if version < n { … }`. The current watermark is **18**
(`connection.rs:155`, `MIGRATION_V18` at `schema.rs:374`). V18's established
shape — which V19 follows — is: run the migration body inside an explicit
`BEGIN; … COMMIT;` batch, do any one-shot repair, and stamp
`INSERT OR IGNORE INTO schema_version (version)` **last**, so a crash before
the stamp re-runs an idempotent body rather than skipping it.

`src-tauri/tests/okf_migration.rs:225` asserts the maximum version. Bumping
the watermark without updating that assertion fails CI by design.

### 1.5 Where manifests are seeded

`seedManifestsIfAbsent` (`src/lib/ontologySeed.ts:35`) writes one manifest per
entity id via `setOntologyManifests(..., { ifAbsent: true })` — atomic
check-and-write, all-or-nothing across the set, never throws.

Its caller `seedOntologyManifests` (`src/lib/wiki.ts:52`) is fed by
`seededOntologyEntityIds()` (`wiki.ts:34`), which returns
`STABLE_ONTOLOGY_ENTITY_IDS` (`tier_fact`, `tier_wisdom`) plus the workspace
tier. **No `ent_*` id is ever in that list.** That is the mechanical cause of
#190's "no row in `llm_wiki_entity_manifests`" observation.

A sharp edge inherited from `seedManifestsIfAbsent`'s doc comment: `ifAbsent`
tests for a *persisted row*, not for an *effective* manifest, so an entity
covered only by the configured seed is reported in `seeded` rather than
`skipped`. Content written is identical.

Known ordering hazard, previously recorded: `setupWiki` runs before the
workspace id resolves, so `getWorkspaceId()` is the `tier_working::default`
placeholder for the whole of `setupWiki`. Any new seeding added in PR 2 must
not assume a resolved workspace id.

---

## §2 — PR 1: Edge writer correctness (#189, #191)

### 2.1 Goal

The librarian's edge writes land with the manifest's canonical `edge_type`
spelling, without same-name endpoint noise, and with `created_at` in
milliseconds; existing seconds-epoch rows are repaired.

### 2.2 Canonicalization

Change `resolve_strict_edge_vocabulary` to return the canonical spelling
alongside the match key. Either shape is acceptable:

- `Option<HashMap<String, String>>` — lowercased key → canonical value, or
- a small `EdgeVocabulary` newtype exposing
  `canonicalize(&self, candidate: &str) -> Option<&str>` and
  `contains(&self, candidate: &str) -> bool`.

The newtype is preferred: `edge_purge.rs:256` and `wiki_graph.rs:648` want
membership only, and a named method keeps the "lowercase before comparing"
rule in one place instead of at three call sites.

`commit_edge_add` then:

1. resolves both endpoints (unchanged),
2. if a vocabulary is present, calls `canonicalize`; `None` → the existing
   drop-and-report branch, unchanged in behavior and message,
3. inserts the **canonical** spelling rather than the candidate.

**The ungated path stays verbatim.** When `resolve_strict_edge_vocabulary`
returns `None` there is no vocabulary to canonicalize against, and the type is
written exactly as proposed. This is correct: canonicalization is a property
of strict mode, and inventing a casing rule for ungated brains would silently
rewrite user data on brains that opted out of the ontology. This is a
deliberate limit, not an oversight.

`edge_purge.rs` and `wiki_graph.rs` keep their current semantics. Their
membership checks must go through the newtype's `contains`, not through raw
map-key access, so the lowercasing rule cannot drift apart from the writer's.

### 2.3 Same-name endpoint guard

After both endpoints resolve to ids, look up `curated_entities.name` for each.
If the names are equal (exact string comparison, after `trim()`):

- do not insert,
- push `item.id` onto `ctx.dropped_edges`,
- warn, naming both entity ids, both names, and the `edge_type`.

This is the same drop-don't-fail contract the two unresolvable-endpoint
branches already use at `commit.rs:1592` and `:1599`: one bad item must not
discard a batch of good facts, and the drop must be visible in the commit
result rather than silent.

Deliberately **not** narrowed to `source_id == target_id`. The Sep 6 noise was
three `supersedes` edges between *distinct* `ent_*` rows that share the name
"Curated Thoughts"; an id-equality guard would not have caught any of them.
True self-edges (`source_id == target_id`) are a subset and are caught by the
same comparison.

Duplicate entities remain the entity-merge pass's problem. This guard stops
the writer from encoding the duplication as graph edges; it does not resolve
it.

### 2.4 `created_at` in milliseconds

- `commit.rs:1639`: pass the millisecond clock instead of `ctx.now_secs`.
- Audit every insert site listed in §1.1. Each must pass a millisecond value.
  Where a site is a test fixture, the fixture value must be `>=
  SEC_VS_MS_THRESHOLD` so it cannot mask a unit regression.
- Add an assertion to at least one test per production insert site that the
  written `created_at >= SEC_VS_MS_THRESHOLD`.

### 2.5 V19: repair existing rows

```sql
UPDATE llm_wiki_edges
   SET created_at = created_at * 1000
 WHERE created_at > 0
   AND created_at < 1000000000000;  -- SEC_VS_MS_THRESHOLD, twelve zeros
```

Requirements:

- **Runs inside an explicit transaction.** Follow V18's shape at
  `connection.rs:159`: `conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;",
  MIGRATION_V19))?`. SQLite gives a bare single-statement `UPDATE` atomicity
  on its own, but the boundary is stated explicitly so that chunking the
  update, or landing another statement beside it later, cannot leave the
  database in an intermediate state. A partial V19 is not an acceptable
  outcome under any future edit to this migration.
- **Stamp last.** `INSERT OR IGNORE INTO schema_version (version) VALUES (19)`
  after the body, matching V18.
- **Idempotent by construction.** A converted row is `>=
  SEC_VS_MS_THRESHOLD` and no longer matches the `WHERE`. Re-entry after a
  crash, or a double-applied replica, cannot multiply a millisecond value into
  the year 31,000. The `created_at > 0` clause leaves sentinel zeros alone.
- **The literal must carry a comment naming the threshold**, and a test must
  assert the migration's constant agrees with
  `schema::SEC_VS_MS_THRESHOLD` — the same protection
  `tests/timestamp_units.rs` already gives the V12 backfill, and the reason
  that test exists.
- Bump the assertion at `okf_migration.rs:225` from 18 to 19, with a comment
  naming this spec.

### 2.6 Read-side guard

Add a normalization helper adjacent to `SEC_VS_MS_THRESHOLD`:

```rust
/// Normalize a possibly-seconds timestamp to milliseconds.
/// Values >= SEC_VS_MS_THRESHOLD are already ms and pass through.
pub fn normalize_epoch_ms(value: i64) -> i64
```

Apply it where `llm_wiki_edges.created_at` is read back into a typed value.
This is defense-in-depth for replicas and bundles that have not taken V19, not
a substitute for it — the migration is what makes the column single-unit.

### 2.7 Tests

| Test | Asserts |
| --- | --- |
| gate accepts case variant, row is canonical | `dependson` proposed under a manifest declaring `dependsOn` → stored row reads `dependsOn` |
| exact-case still writes unchanged | `dependsOn` → `dependsOn`, no rewrite |
| ungated brain writes verbatim | no strict manifest → candidate spelling preserved, whatever its case |
| off-manifest still drops | a type absent from the vocabulary in any casing → dropped, reported, message unchanged |
| same-name pair drops | two distinct ids sharing a name → no row, `item.id` in `dropped_edges`, warning names both ids |
| distinct-name pair writes | control case, still inserts |
| self-edge drops | `source_id == target_id` → dropped by the same guard |
| new edge is ms | freshly committed edge has `created_at >= SEC_VS_MS_THRESHOLD` |
| V19 converts seconds | a seconds row becomes `value * 1000` |
| V19 leaves ms alone | a ms row is byte-identical after migration |
| V19 is idempotent | applying the body twice equals applying it once |
| V19 leaves zero alone | `created_at = 0` is untouched |
| threshold agreement | the migration literal equals `schema::SEC_VS_MS_THRESHOLD` |
| watermark | `okf_migration.rs` max version is 19 |

### 2.8 Verification before merge

Against the live brain: no `llm_wiki_edges` row has
`created_at < SEC_VS_MS_THRESHOLD`, and no row's `edge_type` differs from a
manifest-declared spelling by case alone.

---

## §3 — PR 2: Manifest activation and sweep arming (#190, part 1)

### 3.1 Seed `ent_*` manifests

Two complementary changes, both needed:

1. **Backfill** the `ent_*` ids that already carry edges:
   `SELECT DISTINCT entity_id FROM llm_wiki_edges` filtered to curated ids,
   fed through `seedManifestsIfAbsent` with the extended manifest at mode
   `strict`. One shot, safe to re-run — `ifAbsent: true` makes it a no-op on
   an id that already has a row.
2. **Ingest-time seed** so a newly created curated entity gains its row when
   it is created, rather than reopening the gap for every future entity.

Doing only the backfill leaves the defect scheduled to recur; doing only the
ingest-time seed leaves every existing entity uncovered. Both, or the issue is
not closed.

The workspace-id ordering hazard in §1.5 applies: the backfill must not run
inside `setupWiki`'s pre-resolution window, or it will seed against the
`tier_working::default` placeholder.

### 3.2 Why activating `strict` is safe here

The sweep is **name-only** today (`vocab.contains`, `edge_purge.rs:256`), and
the Sep 8 verification found 201 edges with **0 off-manifest types**. On that
evidence, activation purges nothing.

That is evidence, not a guarantee. **Re-verify against the live database
immediately before merge** — count rows whose `edge_type` is absent from the
resolved vocabulary, per entity — and record the count in the PR. A non-zero
count blocks the merge pending review of exactly which rows would be deleted.

### 3.3 Arm the sweep after each librarian run

Call `purge_off_manifest_edges_all` at the end of the librarian run — the only
thing that writes edges on a nightly cadence. Firing on the write event rather
than on a wall-clock schedule keeps the sweep's cost proportional to writes
and needs no new scheduler.

Contract, matching `applyOntologyChange`'s existing sweep call
(`src/lib/wiki.ts:200`):

- **Never fails the run.** Errors are caught, warned, and swallowed. A sweep
  failure must not discard a completed librarian batch.
- **Logs the purge count** at info level when non-zero.

### 3.4 Purge logging is a hard requirement

Every purged edge is logged individually with its **edge id, entity id,
`edge_type`, `source_id`, and `target_id`**, at warn level, in a single
greppable line per row.

This is a requirement of this PR, not a nice-to-have. Once the sweep runs
automatically, the realistic failure is a user deliberately narrowing a
manifest and the next librarian run silently deleting a class of edges they
did not intend to lose. A count alone is unrecoverable; an exact per-row log
is the difference between "reconstruct the edges from the log" and "restore a
backup". `warn_purged_off_manifest_edge` (`edge_purge.rs:334`) already exists
as the shape to extend — it must carry the endpoint ids, not just the type.

### 3.5 Out of scope: signature conformance

The residual ~10% nonconformance in #190 is edges whose endpoint *node types*
do not match a declared signature. Validating that requires resolving both
endpoints' node types at write time and a policy for legacy violations, and
#190 itself attributes most of the residue to duplicate entities awaiting the
entity-merge pass — meaning a signature-aware sweep would delete rows that are
only nonconformant because a merge has not happened yet.

File a follow-up issue referencing the entity-merge pass. Do not extend the
sweep beyond name checking in this wave.

### 3.6 Tests

- Backfill seeds an `ent_*` id that has edges and no manifest row.
- Backfill is a no-op on an id that already has a row (content unchanged).
- A newly created curated entity gains a manifest row at creation.
- Sweep runs after a librarian run and its count is logged.
- A sweep error does not fail the librarian run.
- A purged edge produces a log line carrying id, entity, type, and both
  endpoints.

---

## §4 — PR 3: Entity-scoped traversal (#190, part 2)

### 4.1 Diagnose first

#190 states that `wiki_traverse_graph` returns silent empty results for
`ent_*` namespaces **because** those namespaces have no manifest row. The code
does not support that causal story:

`wiki_traverse_graph` resolves its vocabulary through
`resolve_strict_edge_vocabulary` (`wiki_graph.rs:648`), which falls back to
`tier_fact` when the curated id has no row (§1.2). A missing `ent_*` row
therefore yields *the tier's* gate — the same gate the writer used to admit
those edges in the first place — not an empty vocabulary and not an empty
result. Meanwhile `commit_edge_add` stamps `entity_id = ctx.entity_id`, the
curated id, so the edges genuinely live in the `ent_*` partition the caller is
asking about.

The likelier cause is seed resolution: `load_live_node` (`wiki_graph.rs:403`)
returns `None` → `wiki_traverse_graph` returns an empty result at
`wiki_graph.rs:637` before any neighbor fetch happens. That is precisely a
silent empty.

**The first task of PR 3 is a reproduction test**, not a fix: seed a curated
entity and edges under an `ent_*` `entity_id`, call
`wiki_traverse_graph(conn, ent_id, seed_id, …)`, and record what happens.

### 4.2 Decision rule

- **Repro fails at the graph layer** → the defect is in `load_live_node` or
  `fetch_neighbors` entity scoping. Fix there; the test is the regression
  pin.
- **Repro passes** → the graph layer is correct and the gap is in the MCP
  argument plumbing (`src-tauri/src/tool_dispatch.rs`,
  `src-tauri/src/mcp_server.rs`): which `entityId` the tool passes down, and
  whether an `ent_*` value survives validation. Fix there, and add an
  integration test at the MCP boundary rather than the graph boundary.
- **Repro passes at both layers** → the issue's premise is stale (plausible:
  it predates PR 1 and PR 2). Close #190's part 2 with the reproduction test
  merged as a regression pin and the finding recorded, rather than inventing a
  fix for a bug that is not there.

This spec deliberately does not pre-commit to a fix. Committing to one before
the reproduction runs would mean designing against a cause the code contradicts.

### 4.3 Acceptance

`wiki_traverse_graph` returns the expected nodes and edges when seeded with an
`ent_*` entity id, exercised through the MCP tool surface, without
`wiki_context` bridging.

---

## §5 — Risks

**V19 rewrites production data.** Mitigated by the threshold bound (only
sub-threshold positives are touched), idempotency (§2.5), the explicit
transaction, and the constant-agreement test. A seconds-epoch value multiplied
by 1000 cannot collide with a legitimate millisecond value from a different
row, because the operation is per-row and order-independent.

**An armed sweep can delete rows.** Mitigated by: the writer fix landing
first, the pre-merge live-database verification in §3.2, the non-fatal error
contract in §3.3, and — the real backstop — the per-row purge logging in §3.4.

**Manifest activation is a mode change on live namespaces.** `strict` on an
`ent_*` id changes the writer's behavior for every future proposal against
that entity. Mitigated by the graceful-degradation branches already in
`resolve_strict_edge_vocabulary`: an unreadable or empty-vocabulary manifest
disables the gate rather than rejecting everything.

**PR 3's size is unknown.** By construction — §4.1 is a diagnosis. If the
repro points at a fix materially larger than the rest of this wave, PR 3 is
re-scoped and re-specced rather than absorbed.

---

## §6 — Explicitly out of scope

- Signature-level conformance validation (§3.5) — follow-up issue.
- The entity-merge pass for duplicate `ent_*` rows (Tessera×6, CT×4, GLM×4).
  This wave stops duplicates from producing edges; it does not merge them.
- #175 (clippy backlog / blocking CI) and #125 (TOCTOU in
  `create_parents_no_symlink`). Unrelated to the edge path; each is a
  standalone change needing no spec.
- Any `core-llm-wiki` / `expo-llm-wiki` change.
