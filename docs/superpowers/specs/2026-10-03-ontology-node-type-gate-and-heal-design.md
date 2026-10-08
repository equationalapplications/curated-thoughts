# Spec: Ontology node-type gate, ontology heal, duplicate merge (wave 1)

**Status:** Draft for review convergence (r19+r20: APPROVE WITH NITS
consecutive, all nits applied. NOTE: inline `rN-*` tags cite the
review round that introduced/verified each rule — they are provenance,
not normative; the normative text is the full sentence around them.)
r21 (2026-10-07, post-Task-1 code alignment): origin-ledger contract
pinned (R2.4.6 — `reason` column, nullable `original_type`, first-origin-
wins), R2.3.5/R2.3.6 scope + reversibility corrected, R2.7.5 archive ×
redirect behavior pinned, R2.7.6 wording, citations re-anchored.
**IMPLEMENTED (2026-10-07, wave 1):** all plan Tasks 0–10 complete on
branch `spec/ontology-node-type-gate-and-heal` (PR #269); tracking
issue #273; wave-2 boundary (§5) remains open. Revision history above
is intact.
r22 (2026-10-08, `/code-review max` wave): R2.3.0 anchor-vocabulary rule
+ fact/task-endpoint opt-out added; R2.7.5 cluster-closed opt-out /
origin ledger / fact dedupe pinned; §2.5 SKIP landing for labeled GUI/LLM
mints clarified (label verbatim, `concept` only when blank).
r23 (2026-10-08, `/code-review ultra` waves 3–4): R2.3.3 a rung-2/3 HOLD
no longer short-circuits the source walk — a later strict source wins,
a climbing sibling reaches rung 4, and only hold + off/no-climb holds;
§2.4.4 `heal --yes` runs the ensure even under unconfirmed drift (drift
blocks retypes/remaps only — the ensure is the recovery the §2.4.5
messages name); the ensure never CREATES a missing `tier_fact` row (the
engine seeds it, §1.6 — recovery messages corrected); R2.7.1
`merge-duplicates --yes` also refuses on degraded/tied config
(`degraded_config`), type agreement uses the vocabulary key, the cycle
census runs on every arm; R2.7.5 resolved survivor→survivor self-loops
are returned with RESOLVED endpoint ids (attributes original); write-time
mint resolution (okf/bundle/helper) walks the whole redirect chain —
read-time stays single-hop error-on-cycle (r2-m6). R2.3.0's
both-endpoints-off SKIP is unchanged: such rows are still judged by the
anchor's vocabulary on read/purge — the E2 design call stays OPEN.
r24 (2026-10-08, controller ruling): E2 RESOLVED as purge-side sparing —
the retroactive off-manifest purge (`edge_purge`) SPARES a row whose
endpoints the write gate would leave ungated today (both off /
no-manifest, a rung-1a opt-out, a strict manifest declaring no edge
types; mirror of `resolve_edge_endpoint_vocabulary` == no vocabulary).
"Off means off" (D8) outranks cleanup: spared rows stay hidden by the
anchor-vocabulary read filter but are recoverable, which deletion is
not. The read filter itself is unchanged (r4-m5 pin). Census deferral:
the operator WAIVED the live-census attach as a merge gate (2026-10-08);
run it post-merge as a verification, not a blocker.
**Date:** 2026-10-03 (written 2026-10-04; r21 2026-10-07)
**Baseline:** `main` @ `9c2281b` (v3.2.0) for the design; line citations
re-anchored (r21) to branch tip after merging `main` @ `0f7ea9f` (v3.3.0).
Citations into `connection.rs` that implementation tasks keep editing are
by SYMBOL, not line (they drifted twice in one day).
EXCEPTION — every line citation inside §2.2 (into `config/mod.rs`,
`ontology_config.rs`, `inference/mod.rs`, `onboard/mod.rs`, `lib.rs`,
`queries.rs`, `tools/src/bin/ct.rs`) describes the PRE-wave-1 code that
plan Task 1 implemented §2.2 against; those stay anchored at `9c2281b`
(`git show 9c2281b:<path>`) because much of the cited code was rewritten
and no longer exists in that form. Citations into files unchanged since
`9c2281b` (`commit.rs`, `entities.rs`, `synthesis.rs`, `bundle_*.rs`,
`okf_migration.rs`, `wiki_graph.rs`, …) are valid at both commits.
**Source of design:** frozen investigation
`records/operations/2026-10-03-ontology-gate-investigation.md` (Rev 50) in the
Equational vault, plus Kurt's rulings of 2026-10-03
(`immutable-source-files/agents/software/curated-thoughts/ontology-gate-rulings-2026-10-03.md`).
This spec restates that design as normative requirements. Citation tags:
`[V]` = verified live against source/DB during the investigation.

**Decisions are CLOSED.** All scope, sequencing, type-mapping, and rename
rulings below were made by Kurt on 2026-10-03 and are constraints, not
proposals. Reviewers should flag implementation conflicts, not re-litigate
the decisions.

---

## §0 — Problem and scope

CT's Active Librarian enforces the strict ontology manifest for **edge
types only**. Node types (`curated_entities.entity_type`) have no gate at
write time and no repair pass at heal time. On the live ThinkPad brain,
49 of 138 live entities (36%) (at investigation freeze; see §1.1 for the corrected live census) carry types outside the declared 18-type
manifest, and nothing in CT corrects them [V: SQL census,
2026-10-03]. Kurt ruled 2026-10-03 this must be fixed by (a) a write-time
node-type gate and (b) ontology repair inside the heal pass — "not only at
write time."

**Two waves (Kurt: "Two waves"):**

- **Wave 1 (this spec):** write-time gate + ontology heal + signed-alias
  remap + local-redirect duplicate merge. Merges are LOCAL-ONLY: the loser
  stays live with a `merged_into` redirect resolved on both read and write
  paths; no facts move; no outbox rows; no bundle-format changes.
- **Wave 2 (separate spec):** outbox graph transport (entities, types,
  tombstones, edges, real re-pointing), bundle format changes, peer
  import-applied redirects. Listed in §5 for boundary clarity; nothing in
  this spec implements it.

Per the accepted §7 register (Kurt: "Yes to all section 7 questions"):

1. Gate + heal + data migration land **TOGETHER** in wave 1; heal performs
   the alias remap on first run behind `--yes`.
2. The duplicate sweep is a **ONE-TIME command** (not a recurring heal
   duty); the deterministic-survivor rule is written so it can be reused
   later.
3. The `ct wiki sweep` node-type extension **folds into** this spec.

---

## §1 — Shared context (facts every implementer must know)

### 1.1 The declared vocabulary and the drift

Declared node types (manifest `tier_fact`: the SHIPPED SEED is 17 —
r7/r8 CORRECTION; the LIVE ThinkPad row is 18 = seed + `concept`, which
an emergent merge added on top [V: re-censused live 2026-10-04, and the
live census now shows 152 live entities with NEW drifted types `job`
(1) and grown counts: concept 33, document 30, agent 10, software 7,
component 3, process 2]). Seed types, verified live against
`@equationalapplications/schema-software-org`:
action, creativework, design_spec, event, handoff, organization, person,
place, procedure, product, project, reference_doc, review, role,
service, session_recap, software_application. `concept` is NOT declared
by any shipped seed — SchemaOrg declares 9 (person, organization, place,
event, project, action, creativework, review, product), also without
`concept`. The live ThinkPad `tier_fact` must be RE-CENSUSED at
implementation start; the seed-set guard below uses subset-match so a
superset row still matches.

In use but NOT declared per the SHIPPED SEED (6): `document` (28 at
freeze, 30 live 2026-10-04), `agent` (10), `software` (6→7),
`component` (3), `process` (2), `job` (1, NEW — r9-m2; no Kurt ruling
covers it → review QUEUE under §2.6.2) [V: live recount 2026-10-04,
sums to 53].
Total drifted 49/138 at freeze, 53/152 live 2026-10-04 (r10-MINOR-1:
the earlier "61/152" was arithmetic error — verified recount:
30+10+7+3+2+1 = 53 drifted; denominator is the live total, 152).
`concept` (33) is declared ON THE LIVE ROW (seed + emergent merge)
though not in the shipped seed.

Drift is NOT legacy: the manifest was seeded 2026-09-06, the first entities
minted 2026-09-29 — every drifted row was born under the open prompt [V].

Root cause: the synthesis prompt offers an open set
(`synthesis.rs:578` and `:591` both contain the literal hint
`"type":"person|project|concept|..."` — the ellipsis invites invention),
and commit stores verbatim
(`db/commit.rs:1383-1386`,
`proposed_type.clone().unwrap_or_else(|| "concept".into())`, INSERT at
1388-1392, no manifest check). Edge types, by contrast, are constrained in
the prompt and re-validated at commit
(`db/commit.rs:290` `resolve_strict_edge_vocabulary`). The asymmetry is the
#158 fix applied halfway.

### 1.2 All entity-creating paths [V]

There are exactly FOUR production entity-insert sites; the gate must cover
all of them through one shared helper (§2.4.2):

1. `create_entity_if_needed` (`db/commit.rs`, private, one caller:
   `resolve_proposal` at `commit.rs:2365`) — LLM/GUI proposal mint path.
   In production, LLM synthesis is the ONLY builder of `NewEntity`
   proposals (`synthesis.rs:807`).
2. `entities::create_entity` (`db/entities.rs:542-561`) — GUI/command path
   (exposed via `entities_api.rs:44`); stores caller's entity_type
   verbatim, defaults blank/None to `"concept"`.
3. `bundle_apply::ensure_entity` (`db/bundle_apply.rs:711-736`) — bundle
   import; hard-codes `'concept'`; never retypes existing rows.
4. `okf_migration` (`db/okf_migration.rs:94`) — hard-codes `'concept'`;
   ids are path-derived `entity::<16hex>` (`okf_migration.rs:28`).

Non-LLM producers can omit `entity_type` (the
`unwrap_or_else("concept")` default fires only there — on the LLM path the
field is REQUIRED and a missing value fails serde parse → retry →
errors.log, `synthesis.rs:84-88`, `:995-1015`).

### 1.3 Duplicate entities [V]

18 distinct names exist as 2+ live entities, 56 rows total (e.g.
Adrian ×10, Tessera ×5). Duplicates arise from punctuation variants too:
`Memory Architecture Intent 2026-09-01` (×3) vs
`Memory Architecture Intent (2026-09-01)` (×2) differ only by
parenthesization. **The "entity-merge pass" cited in comments
(`commit.rs:2040, 2060, 4473`) and the Sept 8 edge-integrity spec does not
exist** — the merge sweep is NET-NEW design. The only merge code in the
tree is `bundle_apply.rs` `ImportMode::Merge` (bundle-import semantics, not
duplicate consolidation).

### 1.4 Entity ids and fleet constraints [V]

- LLM/GUI-minted ids are 12 random bytes (THREE generators, r9-m1:
  `commit.rs:448 generate_llm_id("ent_")` for synthesis mints; GUI
  `generate_entity_id`, `entities.rs:162-166`; bundle Clone mode
  `okf::ids::generate_id("ent_")`, `bundle_apply.rs:17,335` — same
  shape; the shared insert helper (§2.4.2) owns exactly one for the
  MINT paths and also accepts CALLER-SUPPLIED ids, since bundle
  non-Clone imports and OKF path-derived ids bring their own) — NOT
  content hashes.
- OKF-migrated ids are path-derived `entity::<16hex>` — identical across
  hosts with the same vault layout.
- Bundle imports keep source-host ids (`bundle_apply.rs:334-336`) in
  Merge mode; Clone mode mints NEW ids (r9-m1), so Clone imports can
  create same-name duplicates on the importing host — the merge sweep's
  re-entrancy covers them.
- Consequence: two hosts merging the same duplicate pair can pick
  different survivors from locally-differing rows. Survivor selection must
  be deterministic (§2.7.3) and real merge transport is wave 2 (outbox),
  not per-host recomputation.

### 1.5 Sync channels carry no graph types [V]

- The outbox does not carry entities or edges (table contract
  `entries | tasks | events`, `outbox_format.rs:36`; "Edges are not
  replicated" — `db/queries.rs:233`).
- Bundles drop entity_type and tombstones: export selects only
  `id, name, summary` of live rows (`bundle_io.rs:16`); import inserts
  everything as `'concept'` (`bundle_apply.rs:733-734`). Entry-UPDATE
  outbox payloads DO carry entity_id (`outbox_format.rs:35,125-131`) —
  this is the wave-2 re-point carrier, not a wave-1 concern.

### 1.6 Lookup chain and edge cascade today [V]

- The edge-mode cascade is two steps: entity's own manifest row →
  `tier_fact` (`commit.rs:361-382`). There is NO partition leg.
  `tier_working::<hash>` rows are vault-root-keyed
  (`connection.rs:237-242`) and sit OUTSIDE the cascade.
- `off` cannot be distinguished from absent:
  `wiki_get_ontology` returns `mode:"off"` when no row exists
  (`wiki_graph.rs:242-246`), and the cascade deliberately does NOT
  short-circuit on an entity-level `mode:"off"` (`commit.rs:364-369`).
  D8 (§2.1) therefore requires both a real opt-out marker and a
  short-circuit change.
- Today there are ZERO `mode='off'` rows in any production manifest table
  on the ThinkPad brain (all 5 rows strict: tier_fact, tier_wisdom, 3
  tier_working partitions) [V — RR1 census].
- CT today has NO production code that writes manifest rows — every INSERT
  is `#[cfg(test)]`; the `tier_fact` seed came from outside CT [V].
  Fresh brains have no manifest until something seeds one; the no-row →
  tier_fact fallback then applies to nothing. This spec adds the first
  supported writer (§2.2).

### 1.7 Vault-switch and clear-path constraints [V]

- The brain DB is global; knowledge is per-vault
  (`queries.rs:178-182`, issue #213). `clear_vault_tables` deliberately
  KEEPS the tier vocabulary manifest rows (`tier_fact`, `tier_wisdom`,
  `tier_working::%`) — omitted from the delete batch
  `queries.rs:247-269`; asserted by
  `clear_vault_tables_empties_every_clear_row_in_the_d2_matrix`
  (`queries.rs:943`, keep-row assertions `:983-1034`).
- **FIXED by plan Task 0 (r21; was a latent pre-existing bug, r9-M2):**
  the clear now deletes entity-level manifest rows while keeping the
  tier rows (`queries.rs:271-289`). The original finding, kept for
  provenance: entity-level
  manifest rows already leak across vaults (clear list omits
  `llm_wiki_entity_manifests`; path-derived OKF ids are identical across
  vaults). Latent today (no production writer, ThinkPad census found zero
  `ent_*` rows), becomes LIVE the moment wave 1's `ct ontology set
  --entity` ships. The split-clear fix is a PREREQUISITE of shipping
  `--entity` (§2.9.2).
- `switch_vault` has two behaviors (`lib.rs:2022-2089`): restore branch
  (swaps whole DB — manifests effectively per-vault) and clear branch
  (manifests per-host). Both paths need tests (§2.9.2).

### 1.8 The OKF name collision [V]

OKF frontmatter has its own `entity_type:` field with a different enum
(`okf/mod.rs:53-59`). This spec's gate scopes to
`curated_entities.entity_type` ONLY. The OKF field gets renamed in Rust to
`doc_kind` in wave 1 (§2.8) precisely to kill this ambiguity for agents;
the on-disk key is unchanged.

---

## §2 — Wave 1 normative requirements

### 2.1 Ontology mode: semantics

- **D8 (Kurt): "ontology off" means OFF** — no gate, no tier fallback, no
  heal enforcement. A host/directory/entity that opted out must not have
  its entities checked against any other manifest.
- Mode is resolved **READ-LIVE at every gate/heal lookup** — no stamping,
  no stored per-entity mode inherited from ingest time (there is no tier
  stamp mechanism to inherit from; `librarian/mod.rs:226-228` re-resolves
  from config per call — same pattern). A config edit takes effect at the
  next lookup.
- The edge cascade's current deliberate fall-through on an entity-level
  `off` (`commit.rs:364-369`) is CHANGED by this spec: an explicit,
  deliberate opt-out short-circuits (skips edge gating too). A row that
  is present but NOT marked as a deliberate opt-out falls through as
  today. Edge and node gating and heal all use the SAME ordered skip
  rule (§2.3).
- **Edge-gate degradation outcomes are PINNED (unchanged by this spec):**
  strict manifest with an EMPTY edge vocabulary → the edge gate disarms
  with a loud warning (`warn_strict_manifest_declares_no_edge_types`,
  `commit.rs:316-318`); unreadable ontology → disarms with
  `warn_ontology_unreadable` (`:376-378`, PR #78 graceful degradation).
  Both behaviors carry over verbatim for EDGES. The NODE gate has no
  disarm (§2.4.5) — the asymmetry is deliberate: a held node proposal is
  recoverable (facts re-enter), while silently dropping every edge of
  every proposal is not (the comment at `commit.rs:270-281` is normative
  for edges). Pin both with tests.
- **No-manifest rule (fresh installs, CLI-only brains, engines never
  seeded):** the skip ladder's tier_fact rung is "no `tier_fact` row →
  SKIP (no manifest to enforce)" for BOTH edges and nodes — the
  no-row → tier_fact fallback applies to nothing, so gating disarms,
  exactly as the edge gate behaves today (`OntologyLeg::NotStrict` →
  `None`, `commit.rs:375`; cached return `:382`). This is
  the residual-default-requires-manifest rule: "default strict" applies
  only where a manifest actually exists to be violated. Matrix test:
  fresh brain + one LLM mint → one entity, zero held proposals.

### 2.2 Ontology mode: configuration surface

**R2.2.1** New config key `ingest.folder_ontology`: map of vault-relative
path prefix → `off|strict`. SIBLING of `ingest.folder_tiers`
(`config/mod.rs:93`) — a separate map, NOT a fourth `IngestTier` value
(one enum value per prefix cannot say "fully ingest this folder, ontology
off"; merging them would make `off` suppress ingestion, the opposite of
D8) [V: r41-M1]. New `IngestConfig` fields carry `#[serde(default)]`
(r5-m2, narrowed r7-m3): the `folder_ontology` HashMap NEEDS it — a
config missing the key fails STRICT deserialize without it and routes
`load()` to `load_lenient`, silently discarding typed generation edits
(the PR #120 footgun the comment at `config/mod.rs:523-529` warns
about; the `#[serde(default)]` rule at `:248-251` holds) — while the
`Option<OntologyMode>` scalar deserializes to `None` without the
attribute (kept anyway for symmetry). Matrix test: pre-wave-1 ingest
block loads STRICTLY.

**R2.2.2** Resolution reuses the same longest-prefix path-component-boundary
resolver as `tier_for` (`config/mod.rs:106-123`) — as a GENERIC extracted
core (r2-M5): `tier_for`/`tier_for_path` are hard-wired to
`self.folder_tiers` + `IngestTier` and `tier_for_path` short-circuits on
`folder_tiers.is_empty()` (`:133-135`), so a copy/wrap for
`folder_ontology` would resolve every path to the default when
`folder_tiers` is empty (the common case) — silently ignoring every `off`
folder. Required: extract `resolve_prefix<T>(map, path, vault_root) ->
Option<T>` (longest-prefix walk + vault-root relativization — the
relativization today lives at `config/mod.rs:136-144`, r2-m3) with the
empty-check applying to the MAP BEING QUERIED and an unplaceable absolute
path returning `None` (climbs to the next rung) — EXCEPT (r3-M4, trigger
corrected per r4-M3, WIDENED per r9-M2, scoped r15-m2, scope ALIGNED with R2.3.2 per r20-m1: whenever
`folder_ontology` contains ≥1 `off` OR dropped/degraded entry (a
strict-only map with no off prefix has nothing to protect and must
NOT stall mints) and an ABSOLUTE path cannot be
resolved to a mode (relative paths resolve without any root,
`config/mod.rs:137-138`, and are NOT affected) — either the
EFFECTIVE vault root is None (r13-m1: NOT keyed on the
`LoadReport.vault_path_missing` flag — that reflects only the
configured key, set at `config/mod.rs:584/:715/:718`; the trigger is
the CALL-TIME effective root per r7-m2 below — a caller-passed root
with no configured one still resolves) OR the root exists but
`relativize_to_vault` returns None (moved/renamed vault, stale
`documents.path` absolute paths, dead symlink —
`walk_vault.rs:130-143` returns None when no prefix pair matches) —
resolution is REPORT-OR-HOLD (classified `HadEvidenceUnresolved`),
never a silent climb to
strict rungs: a silent climb would `heal --yes`-retype folders the user
marked off, breaking D8. Matrix cases: caller passes None + `vault_path`
configured → resolves normally; `folder_ontology` non-empty + effective
root None + `heal --yes` → zero retypes, rows reported; vault moved +
off prefix + `heal --yes` → zero retypes (r9-M2); **tie rule (r18-m1):**
two `folder_ontology` keys that normalize to the same prefix
(`"ops"` vs `"ops/"`) — the HashMap iterates in random order and
`depth > d` keeps the first-seen, so the winner would flip per run →
the config is DEGRADED — **detected in the shared prefix resolver itself (r19-m1: the salvage path never runs for a strict-parse config, so flags set there cannot catch this)**: `resolve_prefix` collects all keys whose normalized form equals the longest matched prefix; if ≥2 such keys have CONFLICTING values it returns a `Tie` outcome → `ingest_ontology_degraded` + scoped hold per R2.2.4 + a loud diagnostic (same-value ties are harmless and resolve to that value). The SAME tie check covers `folder_tiers` with outcome = MOST
CONSERVATIVE tier wins (`none` < `chunks-only` < `full`) + a loud
diagnostic (r20-m3: `IngestTier` has no Tie variant; conservative-wins
matches the gate's hold-bias) Matrix case:
`{"ops":"off","ops/":"strict"}` → hold, never a silent random winner.
`full` (in
folder_tiers) + `off` (in folder_ontology) on the same prefix must
ingest normally AND skip gating. **Rung-3 carrier (r6-M4):** rung 3
reads `ontology.schema` + `LoadReport.ontology_unparseable` LIVE, so
`IngestPolicy` (`config/mod.rs:151-154`: `tiers` + `vault_root`)
gains explicit fields: `ontology_selection: Option<OntologySelection>`,
`ontology_unparseable: bool`, `ingest_ontology_degraded: bool` +
dropped-key detail (§2.2.4) — NO `effective_root_missing` field
(r7-m2: the effective root is `caller_root.or(configured vault_path)`,
known only at call time); the resolver computes
`vault_root.or(self.vault_root).is_none()` itself —
`ingest_policy_for_db` populates them and its byte-keyed cache
(raw-bytes compare, `config/mod.rs:165/:197-198`, cache store at
`:195-216`) carries them. **Parse from the SAME bytes that key the
cache (r11-m5):** today the cache is keyed on the `:191` read but
`load_lenient` re-reads the file at `:204` — a concurrent write can
store a policy (now carrying degraded flags) under bytes it was never
parsed from. Wave 1 adds a lenient loader taking the already-read
string. A cache test pins that a config whose only change is the
schema block invalidates correctly. Without this carrier
an implementer passing only `report.config.ingest` through cannot
distinguish schema-Off from schema-unparseable — the exact guessing
R2.2.5 forbids.

**R2.2.3** New scalar config key `ingest.ontology_default` (`off|strict`),
the host-wide default, typed `Option<OntologyMode>` with `#[serde(skip_serializing_if = "Option::is_none")]` (r19-m3: without it every `write()` stamps `"ontology_default": null`, and `null` has no salvage meaning) so rung 3 can
distinguish ABSENT (climb to rung 4) from SET (r3-m4); a hand-written `null` parses as `None` = absent. It is a separate
key, not a `""` prefix (the
prefix resolver skips empty prefixes, `config/mod.rs:113-114`).

**R2.2.4** Load-failure rule: `ingest_policy_for_db`
(`config/mod.rs:180`; doc comment `:176-179`) silently returns the DEFAULT
policy on a missing OR malformed config — acceptable for tiers, NOT for
ontology. **Where the degraded state lives (r4-M1, corrected r5-m1):**
NOT on the `LoadReport` — the production writers (`lib.rs:4794/4842/4874`)
call `BrainConfig::load()`, which salvages via `load_lenient` and returns
ONLY `report.config` (`config/mod.rs:544-552`), throwing the report and
any flags away. The degraded state and the record of WHICH keys were
dropped (dropped `folder_ontology` prefixes; whether `ontology_default`
was dropped) live on `BrainConfig` ITSELF as `#[serde(skip)]` fields,
set inside `load_lenient` (r5-M5: deliberately NOT the
`raw_generation` pattern — those are
`#[serde(skip_serializing_if = "Option::is_none")]`,
`config/mod.rs:280-287`, and are set only in `load()`'s fallback arm;
`raw_ingest`/`ingest_ontology_degraded` MUST be `#[serde(skip)]` and
set in `load_lenient`, because most writers — privacy `:55-59`, vault
config `:80-140`, inference `:259-271`, onboard `:183-207` — go
`load_lenient(...).config` → write, never through `load()`). Every
writer family is covered by a matrix case (r5-M5, inventory
re-derived r12-M4 via `rg 'BrainConfig::load(_lenient)?\('` — 38
call sites): `load()`→`write()` (lib.rs setters, vault
`config.rs:44` `load_strict_or_fresh`, inference
`config.rs:72`), `load_lenient`→`write()` (privacy toggle),
onboard merge. Both entry points set the skip-fields identically
(`raw_ingest` is set in `load_lenient`, which `load()`'s fallback arm
reaches), so the matrix drives each family through ITS OWN entry
point (r15-m5: no stored counts — the implementer runs the rg and works from its output). Matrix test drives
`load()` → `write()` (not `load_lenient` directly). Writer rules:
- **Writer inventory rule (r5-M1):** every `BrainConfig::default()`-then-
`write` site is either (a) onboarding-with-explicit-consent or (b)
CHANGED to load → reset only its own block → write. Today
`inference/mod.rs:240-241` and `:272-273` roll back provider changes by
writing `BrainConfig::default()` wholesale — that erases the whole
`ingest` block (folder_ontology opt-outs gone → next `heal --yes`
retypes opted-out folders, breaking D8) AND `ontology.schema`. Fix:
both rollbacks become `load_lenient → cfg.generation = default →
write` (preserving ingest/ontology), or carry over the on-disk
ingest/ontology blocks. Audit rule: `rg 'BrainConfig::default'` write
sites reviewed; matrix test: opt-out set → provider-init failure →
opt-outs still present. (onboard `:187-199`, vault `:46`, and
`lib.rs:4303` — startup error arm, diagnostics only, NOT a write site —
are audited, allowed; r7-m5.) PLUS the `tools/` CLI writers (r14-m2):
  `ct trust` (`tools/src/bin/ct.rs:820/:852/:934`) and
  `walk_list.rs:45` do `load()` -> mutate -> `write()`; matrix case:
  `ct trust` while degraded -> raw ingest survives byte-for-byte (the
  `write()` protection covers them generically; the test pins it).
  Audit command with exact scope: `rg 'BrainConfig::load(_lenient)?\('
  src-tauri/src tools/src` — work from the rg output itself (r15-m5:
  no stored counts — a previously stored count was stale).
- Non-UTF-8 bytes count as DEGRADED, not missing (r12-m5:
  `ingest_policy_for_db` reads raw bytes at `config/mod.rs:191` while
  `load_lenient` uses `read_to_string` — a UTF-8 error surfaces as an
  `Err`, i.e. load-failed).
- No db path (in-memory/path-less DB, `config/mod.rs:184-189`) →
  resolve normally, no degraded flags (r14-m6: no path = no config
  file to read; test pins in-memory DBs behave as default-policy, not
  degraded).
- Missing config (`ErrorKind::NotFound` ONLY — r2-M3: today the
  `fs::read` at `config/mod.rs:191-193` swallows EACCES/EISDIR and every
  other I/O error as "missing", while `load_lenient` at `:578-588`
  already distinguishes them; `ingest_policy_for_db` must return default
  ONLY on NotFound and set the `ingest_ontology_degraded` flag on any
  other read error) → resolve normally (no map = no explicit ontology
  config; rungs (3)–(4) then decide, per R2.2.5 and §2.3). Matrix case:
  unreadable (EACCES) config + `heal --yes` → zero retypes.
- Load-FAILED (parse error, or the lenient-salvage
  `ingest_ontology_degraded` flag below is set) → gate stays ON,
  destructive heal REFUSED (report-only with a loud error naming the
  config problem). Matrix case: corrupt config + `heal --yes` → zero
  retypes.
- **Lenient salvage MUST cover the new keys.** Today the block-level
  salvage path salvages ONLY `folder_tiers` entry-by-entry and returns
  `Ok` (`config/mod.rs:830-851`) — once `folder_ontology` and
  `ontology_default` live in `IngestConfig`, a single bad value would
  otherwise salvage to an EMPTY map with a successful load: every
  directory resolves strict and `heal --yes` retypes formerly-off
  folders, and the next `BrainConfig::write` permanently erases the user's
  opt-outs. Required: salvage `folder_ontology` entry-by-entry (drop only
  the bad entry), drop-one for `ontology_default`, an
  `ingest_ontology_degraded` flag + dropped-key detail on `BrainConfig`
  (`#[serde(skip)]` fields, per the r4-M1 rule above — NOT on
  `LoadReport`; the existing `LoadReport.ontology_unparseable` is
  declared at `config/mod.rs:311`, initialized at `:567`, refers to the
  `ontology.schema` block and must not be conflated;
  r2-m5/r7-m1), and `ingest_policy_for_db` exposing that flag instead of
  collapsing it to the default. Values are LOWERCASE and case-sensitive
  (`off` | `strict`; r2-m8): a hand-edited `"Off"` is a bad value →
  dropped by salvage + `ingest_ontology_degraded` set (never silently
  parsed, never salvaged into a legal value). **One salvage function
  (r3-m1, corrected r4-m7):** the second salvage path,
  `pub fn ingest_from_value_lenient` (`config/mod.rs:350`), has NO
  production caller (its only use is the test at `config/mod.rs:1240`)
  — DELETE it (AND its doc comment at `config/mod.rs:343-349`, which
  goes with it) and point the test at `load_lenient` via a tempdir
  `BrainPaths` + config-file fixture, rather than a raw `Value`
  (r20-m5). An entirely
  unparseable ingest block — RESTRUCTURED per r15-M1: salvage each
  key INDEPENDENTLY, present-or-not (`folder_tiers` is missing on a
  typical config today, and the `:846-849` "all folders full" branch
  fires whenever `folder_tiers` is not an object — under the old
  wording that branch set the flag and held EVERY mint on any brain
  without `folder_tiers`, contradicting the scoped-hold rule below).
  Required behavior: each of `folder_tiers` / `folder_ontology` /
  `ontology_default` salvages only if its key is present; the
  "entirely unparseable → GLOBAL `ingest_ontology_degraded`" case is
  reserved for an `ingest` value that is not an object at all
  (r3-m5's rule survives for exactly that case). Matrix case: NO
  `folder_tiers` + one bad `folder_ontology` entry → only that
  prefix is held, everything else gates normally. **Accepted
  pre-existing behavior (r5-m3, documented):** an ingest typo routes
  `load()` to lenient, and `lib.rs:4794`-style sites then drop typed
  generation edits (raw_* set on that arm) — pre-existing for tier
  typos; wave 1 adds a new trigger but does not fix it here (fixing the
  raw_* arms is out of wave-1 scope; noted for the implementer).
  Matrix case: a
  bad `folder_tiers` value must NOT erase or disable `folder_ontology`
  entries. Struct-literal sites the compiler will flag when fields are
  added (r5-m4): `config/mod.rs:1248-1250`, `librarian/mod.rs:582`.
- **Write-side protection (r2-M4 + r4-M2 — TWO mechanisms, not one).**
  Mirroring the two existing patterns in config/mod.rs:
  - `preserved_ingest` (the `preserved_wiki` pattern,
    `config/mod.rs:1002-1011`; extracted in BOTH load arms — `load()`
    success arm `:508-521` AND `load_lenient` `:703`, r13-m2:
    `IngestConfig` has no `deny_unknown_fields` (`:87`), so unknown
    sub-keys parse clean and `write()`'s whole-block `to_value` at
    `:1015` drops them unless both arms capture them; test: unknown
    `ingest.future_key` survives BOTH `load()`→`write()` and the
    lenient fallback): UNKNOWN ingest sub-keys only, ALWAYS
    kept (r3-m2) — future-binary keys survive every load/write.
  - `raw_ingest` (the `raw_generation` pattern): the VERBATIM ingest
    block, set ONLY when salvage dropped something (degraded load);
    while set, `write()` LEAVES THE ON-DISK `ingest` VALUE IN `root`
    UNTOUCHED (`write()` already rebuilds from the on-disk root,
    `config/mod.rs:900-911` — r15-m1: emitting the load-time captured
    copy would put the user's OLD broken block back over their
    in-between fix; leaving root untouched is simpler and has no
    TOCTOU window) — this is the only mechanism that can preserve a
    dropped entry INSIDE the known `folder_ontology` map — and
    `ct ontology set` REFUSES while it is set ("fix config first";
    r3-M3 — the refusal stands because typed ingest mutations are
    DISCARDED: `write()` leaves the on-disk `ingest` untouched while
    degraded, r15-m1). On a healthy config `raw_ingest` is never set, so
    `ct ontology set` persists normally (matrix test: healthy config
    + `ct ontology set --dir x --mode off` → survives a write).
    Per-key merge is explicitly NOT pursued.
    **ALL ingest-mutating writers refuse while `raw_ingest` is set**
    (r8-m3, reworded r17-m1 per r15-m1: `write()` leaves the on-disk
    `ingest` value untouched while `raw_ingest` is set — typed
    mutations are DISCARDED, not re-emitted from the captured copy), so
    any typed `folder_tiers` mutation made while degraded would be
    silently discarded — the PR #120 footgun again; the refusal is not
    `ct ontology set`-specific.
  - Matrix tests (driving `load()` → `write()`, per r4-M1): degraded
    load + write → raw block survives byte-for-byte; `ct ontology set`
    while degraded → refused; every writer family covered (lib.rs
    setters, privacy toggle, onboard merge, vault config — r5-M5).
  - Second erasure path — DOWNGRADE: a binary older than wave 1 re-serializing
  `IngestConfig { folder_tiers }` silently deletes `folder_ontology`/
  `ontology_default`: noted as a ROLLBACK HAZARD in §3 (older binaries
  erase opt-outs on any config write; restore-from-backup is the
  recovery).
- **Scoped hold (r3-M2).** A mint whose resolution would REACH a
  dropped/failed part of the config is HELD — not every mint on the
  brain. `ingest_policy_for_db`'s salvage knows WHICH key was dropped:
  a dropped `folder_ontology` entry holds only mints whose source path
  falls under that prefix; a dropped `ontology_default` scalar holds
  every mint that would climb to rung 3; a `folder_ontology` value that is not an object at all, a top-level
  parse failure, or a non-NotFound I/O error holds everything (the
  global case; r6-m6). Matrix
  case: `"Off"` typo under prefix `x` → mints under `x` held, mints
  elsewhere gate normally.
- Write-side symmetry: with the load failed, the gate must NOT silently
  degrade mints to the fallback. A mint whose resolution would have
  reached the folder map is REFUSED/HELD like an SG6 no-fallback refusal
  (proposal held, bundle import aborts, GUI error, migration aborts —
  see §2.5). Matrix case: corrupt config + ingest under a formerly-off
  prefix → proposal held, `entity_type` untouched.

**R2.2.5** Interaction with the existing `ontology.schema` selection
(`config/mod.rs:242-244`, `ontology_config.rs:15-24`:
`SchemaOrg | SchemaSoftwareOrg | Emergent | Off`, user-facing onboarding
choice). The two settings are DIFFERENT layers and must not silently
override each other:
- `ontology.schema` selects WHICH manifest the TypeScript engine seeds
  (`createWiki`) — it does not gate writes and it does not opt any
  directory out of gating.
- `ingest.ontology_default` / `folder_ontology` govern whether the
  node/edge gate RUNS (the §2.3 ladder), independent of which manifest
  was seeded.
- Mapping rule (r5-M2 — LIVE resolution, no one-shot migration): rung 3
  resolves from `ontology.schema == Off` LIVE whenever
  `ingest.ontology_default` is ABSENT (`None`) — same read-live
  principle as everything else in this spec; there is NO one-shot
  config migration and NO marker (r5-M4: the earlier marker design is
  dropped). A later schema switch in settings (`lib.rs:4795`) or
  re-onboarding (`onboard/mod.rs:205`) therefore reaches the gate
  immediately. If the user sets `ingest.ontology_default=strict`
  explicitly, that explicit choice wins over the schema-derived
  default. If the `ontology` block is unparseable
  (`LoadReport.ontology_unparseable`, field declared at
  `config/mod.rs:311`, initialized at `:567`; the code leaves
  `config.ontology` at default and only sets the flag,
  `config/mod.rs:794-803`; r20-m4: REWRITE the stale doc comment at
  `ontology_config.rs:36` — "Unparseable values load as `None`" is
  wrong, an unknown variant FAILS the block deserialize and sets the
  flag — and add `skip_serializing_if = "Option::is_none"` to
  `schema:` alongside the `raw_ontology` work), rung 3 treats schema intent as UNKNOWN →
  DEGRADED (r15-M2): mints that would reach rung 3 are HELD
  (report-or-hold) and DESTRUCTIVE HEAL IS REFUSED — NOT a silent
  climb to rung 4 (seeds use `ifAbsent`, so an Off-brain once strict
  still carries a strict `tier_fact` row; a climb would let
  `heal --yes` retype entities the user opted out of — D8 violation,
  contradicting R2.2.4's own load-failed rule). Matrix case:
  unparseable schema + absent `ontology_default` + strict `tier_fact`
  + `heal --yes` → zero retypes. A diagnostic is still emitted (never
  guesses schema intent, r2-m5). **Persistence of the degraded state
  (r16-MAJOR-1):** `write()` re-serializes the typed ontology block
  (`config/mod.rs:973-983`, `:1001`); after an unparseable load that
  block is `OntologyConfigBlock::default()` and `schema: Option` has
  no `skip_serializing_if` (`ontology_config.rs:37-38`); kept unknown
  keys exclude `schema` (`config/mod.rs:682`); there is no
  `raw_ontology` — so ANY unrelated writer (`approve_link`/
  `revoke_link` `lib.rs:4842/4874`, the privacy toggle, `ct trust`)
  overwrites the bad value with `{"schema": null}`, the next load
  sees `schema: None` with no flag, rung 3 climbs to rung 4 (strict),
  and `heal --yes` retypes the opted-out brain. Required: a
  `raw_ontology` / `#[serde(skip)]` degraded flag mirroring the
  `raw_ingest` mechanism — when the ontology block failed to parse,
  `write()` leaves the on-disk `ontology` value untouched;
  `set_ontology_selection` is a deliberate schema change, so it gets an
  ESCAPE HATCH (r17-MAJOR-2): `BrainConfig::replace_ontology(sel)`
  clears the `raw_ontology`/degraded skip-fields, sets the typed
  ontology block, and writes — bypassing the leave-untouched rule.
  Callers: `set_ontology_selection` (`lib.rs:4794`) AND the onboarding
  merge (`onboard/mod.rs:205` — otherwise re-onboarding is caught by
  the same discard). No other writer may clear the flag. Matrix
  case: unparseable schema → `approve_link` → next load still
  degraded → heal still refused. `Emergent` ("engine may propose new
  types") is
  NOT an opt-out from gating: emergent types still resolve through the
  §2.3 ladder and the §2.4.4 degrade ladder; the manifest-seeding
  behavior of `Emergent` is unchanged. **No-fallback stall cannot arise
  on Emergent/off brains (r9-m4 research answer):** `manifestFor`
  returns `null` for `emergent`/`off` (`ontology.ts:47-56`), so their
  `tier_fact` starts with NO row (engine resolution falls to
  `{mode:"off", manifest: empty}`); the ladder short-circuits at the
  tier_fact rung (SKIP) and §2.4.5 never fires. The stall needs
  mode=strict + no usable vocabulary, which only the
  mode-vs-vocabulary rule (r9-M1) can produce — and that is SKIP +
  warning, not §2.4.5. Matrix tests: one per
  `OntologySelection` value × `ingest.ontology_default ∈ {absent, off,
  strict}`.

**R2.2.6** Path normalization: `documents.path` stores ABSOLUTE paths
while `folder_ontology` keys are vault-relative — the resolver
relativizes against the vault root first (the relativization lives at
`config/mod.rs:136-144`; see R2.2.2's generic core), the same
normalization `tier_for_path` performs [V: r45-m4].

**R2.2.7** Lifecycle (deliberate, do not "fix"): ontology mode lives in
the brain config as a vault-root-RELATIVE prefix map, so it survives
vault switches and applies to whatever vault layout matches those
prefixes next. This DIFFERS from `folder_rules` (per-vault, deleted by
`clear_vault_tables` — `queries.rs:253`, asserted `:855`) [V: r41-m1].

**R2.2.8** Drift watermark: a config-hash watermark (hash of the
ontology-relevant inputs the ladder reads LIVE — `folder_ontology` +
`ontology_default`, and WHENEVER `ontology_default` is absent, the
effective rung-3 inputs: `ontology.schema` + `ontology_unparseable`
(r8-M4: a schema Off→strict switch otherwise fires no report while
every Off-minted entity's disposition silently flips); originally
r2-m4: hashing the whole `ingest` block makes every `folder_tiers` edit
trigger a false ontology drift report; stored in `llm_wiki_meta`).
**Canonical hash input (r18-MAJOR-1):** the hash input is a CANONICAL
serialization — `folder_ontology` is a `HashMap` (`config/mod.rs:93`)
whose iteration order is randomized per process, so
`serde_json::to_vec` of it yields a different byte string every run and
the watermark would fire drift on nearly every heal. Required: keys
normalized like `tier_for` does (`\` → `/`, trim `/`), then SORTED
(BTreeMap); fixed field order; `None` and absent serialized
identically; **the hash is computed in BOTH states (r20-m2): tie-degraded input =
`{degraded: true, <conflicting keys EXCLUDED>}`; healthy input = the
normalized map — entering or leaving tie-degraded always changes the
hash and fires a report** (normalization would otherwise collapse
`"ops"`/`"ops/"` into one entry whose surviving value depends on
insertion order); matrix test: same map built in two insertion orders
→ identical hash; tie created → hash changes → report fires. It exists ONLY to
trigger heal's drift
report — heal flags entities whose gated/retyped types were decided under
a since-changed directory mode, and `heal --yes` offers remediation. No
silent mutation happens on a config edit. **Writer (r3-M6):** heal
computes the live ontology-config hash on EVERY run and compares against
the watermark; a successful `heal --yes` run (user having confirmed the
echoed old hash against the live one, per the CLI form below) STORES
the new watermark — heal is the only
writer that UPDATEs the row; the gate/heal RESOLUTION path writes
the INITIAL row ONLY (`INSERT OR IGNORE`, best-effort, skipped on
read-only connections like the ensure — r14-m1). No
`BrainConfig::write`/DB-handle coupling and no
dependence on an unrelated config write to clear a stale state.
**Restore interaction:** the
watermark lives in the DB but the config is per-host — `switch_vault`'s
restore branch swaps in a backup's `llm_wiki_meta`, which can bring a
stale watermark and trigger a false drift report. The drift report is
report-only (never auto-mutating), so a false report is acceptable noise;
the report must ECHO the watermark it compared against (old config hash +
timestamp vs. the live hash it just computed) so Kurt can see the
mismatched provenance and confirm. The ONLY way to clear a spurious
report is a confirming `heal --yes` (comparing the echoed OLD hash
against the LIVE computed hash — r4-m1: there is no "confirming config
read"; heal is the sole watermark writer per above; CLI form:
`heal --yes --confirm-drift <old-hash>` (the echoed old hash as the
argument, copy/paste from the report — r6-m5; stores the new
watermark and proceeds) or `heal --yes --waive-drift <old-hash>`
(r12-m3: acknowledges the mismatch and PROCEEDS WITHOUT retypes or
watermark storage — for "I know, I'll fix the config later"; the
waive event is recorded in the heal summary output only, not
persisted). FINAL RULE (r8-m4,
wording cleaned r10-MINOR-5): an unconfirmed drift report blocks EVERY
destructive `--yes` action — retypes, alias remaps, merges — unless
`--confirm-drift <old-hash>` or `--waive-drift <old-hash>` is given.
**First-run behavior
(r7-m4):** no watermark row exists → NO drift report fires (no prior
state to differ from); the run proceeds like a normal heal (destructive
actions still behind `--yes`) and stores the initial watermark on
success. The INITIAL watermark is ALSO stamped at the first gate or
heal RESOLUTION (piggybacking on the §2.4.4 ensure's resolution-time
hook, r13-MAJOR-3) so gate-time degrade decisions are always made
under a recorded config hash — else weeks of strict-mode degrades are
invisible to the first `heal --yes` after a flip to off. Zero ledger
rows -> still "no report" (nothing to differ from). Test: first-ever heal on a fresh brain → no drift section,
watermark row present after. Pinned by test:
restore-branch swap + unchanged config → drift report fires (echoing the
old watermark), and `--yes` without explicit confirmation of the echoed
pair refuses.

### 2.3 The ordered skip rule (single decision procedure)

Applies to edges AND nodes AND heal. Resolve in order; first hit decides:

1. **Entity-level opt-out / row** → decided at the entity, in order
   (r12-M1 restated: the opt-out marker lives in the CT-owned
   `ct_entity_optouts` table, NOT on the manifest row — §3):
   (a) `ct_entity_optouts` row for this id → SKIP (checked FIRST,
   independent of any manifest row);
   (b) strict manifest row present → GATE;
   (c) manifest row UNREADABLE (`OntologyLeg::Err`) → REPORT-OR-HOLD
   for nodes and heal (r4-m4: an unreadable row may BE the user's
   deliberate choice — climbing to strict rungs would let `heal --yes`
   retype an opted-out entity, breaking D8); EDGES keep today's
   behavior (fall through to tier_fact, `commit.rs:369`; note today's
   entity-leg `Err` ALSO falls through WITHOUT a warning — only the
   tier_fact leg warns, `:376-378` — the report-or-hold rule above
   tightens this for nodes/heal only);
   (d) row present but
   UNMARKED (neither) → NOT a
   configuration error — fall through to rungs 2–3, then tier_fact
   (§2.9.3's "keeps the tier_fact gate" means the RESIDUAL tier_fact
   rung after the climb, r3-m6). (A strict entity row inside an off
   directory is GATED — matrix case.)
2. **Else** resolved `folder_ontology` mode for the entity's source
   directory → off = SKIP, strict = GATE.
3. **Else** host default `ingest.ontology_default` → off = SKIP,
   strict = GATE.
4. **Else** cascade to `tier_fact` (strict, unless itself an unmarked off
   row per §2.3.1; r16-MAJOR-2: the row UNREADABLE or its
   `manifest_json` malformed → REPORT-OR-HOLD for nodes and heal
   (consistent with rung 1(c)), with a LOUD diagnostic — never a
   silent skip (that is the §2.1 disarm the node gate forbids) and
   never a guess. Matrix case: corrupt `tier_fact` `manifest_json` +
   one LLM mint → held, facts kept; `heal --yes` → zero retypes.)

**R2.3.0 Edge endpoints (r2-M9, moved below the ladder — r15-m5/r17-m4:
   the ladder item collided with R2.3.5's ledger rule):** when an EDGE's source and target entities
   resolve to DIFFERENT modes, GATE the edge if EITHER endpoint resolves
   strict (strict-wins across endpoints; an off directory shields its own
   entities from CONTRIBUTING obligations but never downgrades an edge
   the strict side makes checkable — same asymmetry as §2.3.3). If both
   endpoints resolve off / no-manifest → SKIP. Matrix test per
   direction. **Read-path cost pin (r4-m5):** rungs 2–3 apply ONLY to
   WRITE-time gating and heal — read/traversal paths
   (`cross_partition_traverse` etc.) keep today's tier_fact-only
   behavior, so the `StrictVocabCache` memoization
   (`commit.rs:348-351`) stays valid and no per-traversal source
   resolution is introduced. Edge gating at write time resolves the
   endpoints' sources once per proposal (per-proposal memoization, not
   per edge — memoized per OWNING entity, so N fact endpoints of one
   entity resolve its ladder once).
   **Anchor vocabulary (r22):** the edge row is anchored to the PROPOSAL
   entity, and the read filter and the off-manifest purge judge a row by
   that entity's strict vocabulary. So when the endpoint gate fires under
   an endpoint's vocabulary, the type must ALSO be declared by the
   anchoring entity's strict vocabulary (when it has one) — conjunctive,
   never loosening — or the write gate would admit an edge that is hidden
   on read and destroyed by the next sweep. **Fact/task endpoints (r22):**
   rung 1a's opt-out is checked on the endpoint's OWNING entity (the same
   mapping the ladder walks), not only on the raw fact/task id.
   **E2 resolution (r24, controller ruling 2026-10-08):** a SKIP-written
   row (both endpoints off / no-manifest, or an opt-out) is NOT destroyed
   by the retroactive off-manifest purge — the sweep spares exactly the
   rows the write gate would produce verbatim today, so "off means off"
   holds retroactively too. Spared rows remain subject to the
   anchor-vocabulary read filter (hidden until the anchor's manifest
   declares the type or the mode changes — recoverable, r4-m5 pin
   unchanged).

Additional rules:

- **Mode vs vocabulary (r9-M1).** The ladder decides the MODE only.
  The degraded-config scoped hold (§2.2.4) applies only where a
  vocabulary would actually gate (r10-MINOR-2): the vocabulary check
  runs FIRST — no vocabulary row (SKIP per §2.1) makes a config hold
  moot, so fresh brain + `"Off"` typo + no `tier_fact` → SKIP, mint
  succeeds (test 1a spirit). Hold fires only when the mint would
  otherwise be gated/retyped.
  The vocabulary comes separately: the entity's own manifest row if it
  is strict, else `tier_fact` (`commit.rs:361-382` — today's only two
  vocabulary sources; rungs 2–3 supply a mode, never a vocabulary).
  Consequence: mode = GATE via rungs 2–3 + no vocabulary row → SKIP +
  census warning (not a §2.4.5 error — mirroring §2.1's no-row SKIP).
  Matrix case: fresh brain, `ct ontology set --mode strict`, one LLM
  mint → SKIP + warning, mint succeeds.

- **R2.3.1** `tier_fact` itself being an UNMARKED `off` row → SKIP for
  both edges and nodes (edges: exactly today's `NotStrict` → `None`
  fall-through, `commit.rs:375`), PLUS the §2.9.3 census warning. It is
  NOT a configuration error (r2-M2): "no manifest to enforce" and "off
  manifest row" are the same SKIP outcome; only §2.4.5's manifest-error
  cases (strict + zero usable types / no fallback key) hold mints.
- **R2.3.2 Source-directory resolution.** `source_ref` values are either
  `librarian-<hex>` tokens (evidence JSON in `librarian_evidence`,
  resolved via `evidence_json_for_entry`) or plain refs; the resolver is
  `source_docs_from_ref` (`entities.rs:201`) → chunks → `documents.path`
  by content_hash [V]. Heal uses the SAME resolver core as the entity
  reader (no second implementation); **the core returns
  `Result<SourceResolution>` with `Resolved(paths)`,
  `HadEvidenceUnresolved`, and `NoEvidence` kept DISTINCT (r2-M6)** —
  today an empty Vec hides all of them: malformed evidence JSON
  (`entities.rs:223-225`), legacy-chunk-id-only evidence (`:236-241`,
  skipped on purpose), stale `content_hash` (no match), and a DB fault
  in the `query_row` at `:250-251` (`.unwrap_or(None)`) — BOTH swallow
  sites (`:213-217` and `:251`) propagate errors. **Classification
  (r3-M5, corrected r5-M3 — classify on EVIDENCE SHAPE, not token
  shape):** a source is `HadEvidenceUnresolved` if it has a parseable
  `evidence` array with at least one entry that did not resolve (this
  covers NON-librarian refs parsed as inline evidence JSON,
  `entities.rs:220-228`, whose `content_hash`es go stale exactly like
  librarian evidence — a stale inline hash + off folder + `heal --yes`
  must NEVER retype, matrix case pinned), OR it is a librarian-shaped
  token whose evidence row is MISSING (`Ok(None)` at
  `entities.rs:217`). The classification TABLE below (R2.3.2a) is NORMATIVE (r11-M1); this prose does
  not restate it — in particular a plain non-JSON path ref is
  `HadEvidenceUnresolved` per the table, NOT `NoEvidence` (the
  pre-r10 prose here was wrong). Per-fact results combine
  per entity CONSERVATIVELY: if ANY source fact resolves
  `HadEvidenceUnresolved`, the whole entity is report-only for heal —
  BUT only where an unresolved source could change the outcome
  (r11-M2): report-only applies when the resolved `folder_ontology`
  map contains at least one `off` entry (or a degraded/dropped entry).
  With an EMPTY map (no off anywhere — the ThinkPad today) an
  unresolved source cannot flip any mode, so the ladder decides
  normally and the alias remap proceeds (register item 1 preserved).
  Matrix cases: empty map + stale hash + `heal --yes` → remaps apply;
  map with an `off` entry + stale hash → zero retypes.
  Heal treats `HadEvidenceUnresolved` as REPORT-ONLY (UNDER THE R2.3.2
  SCOPE RULE above — on a brain with an empty `folder_ontology` map and
  no degraded flag, remaps proceed; r18-m2) even when there is
  no ledger row (explicitly covering the 53 pre-wave-1 drifted entities (53/152 live, r12-m1),
  all minted before the ledger exists — the ledger fallback alone would
  strand them into auto-retype via rung 4), never treats a DB fault as
  "no source"; the reader wraps the core with degrade-on-error for
  display.
- **R2.3.2a Source classification — COMPLETE TABLE (r9-M4; ORDER
  pinned r20-m7: the shared resolver checks the librarian-token shape
  FIRST (`entities.rs:209-222`), then JSON-parses the rest — keep the
  code's order; a `librarian-<hex>` token is never valid JSON so the
  two orders agree today, but "one resolver" must not grow a second
  ordering). Every
  input shape maps to exactly one outcome; anything unclassified
  defaults to `HadEvidenceUnresolved` (never a silent climb):
  | Source shape | Outcome |
  |---|---|
  | Librarian token, evidence row present, JSON parses, ≥1 entry unresolved (stale hash, legacy chunk-id) | `HadEvidenceUnresolved` |
  | Librarian token, evidence row present, JSON parses, EVERY entry resolves | `Resolved(paths)` (the common healthy case — R2.3.3 strict-wins and all gated flows depend on it) |
| Inline (non-librarian) JSON `evidence` array, EVERY entry resolves | `Resolved(paths)` |
| Inline (non-librarian) JSON `evidence` array, >=1 entry unresolved | `HadEvidenceUnresolved` |
| Librarian token, evidence row present, JSON MALFORMED (`entities.rs:223-225`) | `HadEvidenceUnresolved` (report-only; NOT a DB fault — unified per r11-m1 with the row below) |
  | Librarian token, evidence row MISSING (`Ok(None)` at `:217`) | `HadEvidenceUnresolved` |
  | JSON object with `"evidence": []` (e.g. the V20 doomed-row shape `{"proposal_id":null,"evidence":[]}`) | `HadEvidenceUnresolved` (provenance deliberately nulled; live census: 0 such refs on live facts today, pinned so it stays 0) |
  | JSON object with NO `evidence` key | `HadEvidenceUnresolved` (unknown provenance shape) |
  | Any non-JSON ref that is not an explicitly known non-provenance value — INCLUDING plain path refs (`documents/notes.md` — the shape seeded by the `documents/notes.md` test fixture in `connection.rs`'s source-ref canary test), truncated inline JSON (`{"evidence":[{"content_hash":"ab`), and free text | `HadEvidenceUnresolved` (r10-M2: wave 1 does NOT resolve path refs — `source_docs_from_ref` has no path-resolution branch today; adding one is out of scope — and a naming ref must never silently climb) |
  | Ref ABSENT (`None`) — today including the V20 sentinel, which MIGRATION_V18 already NULLed (`connection.rs:345-356`) — or any other explicitly-listed non-provenance value | `NoEvidence` (nothing claimed; DISJOINT from the empty-evidence row above: empty array = claimed-but-unresolved, NULL = never claimed) |
  | Any DB fault | propagate / stop heal — malformed evidence JSON is NOT a DB fault: it is `HadEvidenceUnresolved` (report-only), r10-MINOR-4. The `:217` arm splits THREE ways (r14-m4): `Ok(None)` -> unresolved, `Err` -> propagates, `Ok(Some)` -> parses; the SECOND display wrapper is `wiki_graph::wiki_context` (why `source_docs_from_ref` is `pub(crate)`, `entities.rs:198-201`), degrading on error like the reader |
  Unclassified → `HadEvidenceUnresolved`. Rows are DISJOINT (r10-M2):
  shape tests apply in order — None/known-non-provenance → JSON parse
  → librarian token → else unresolved — so every input matches exactly
  one row. A bare
  `ct ontology set --mode off` host therefore leaves a
  GUI-minted `character` UNGATED — that is the D8 case (matrix test
  pinned). "Default STRICT" is only the RESIDUAL rule after rungs (1)–(3)
  find nothing: gating is the default, exemption always explicit.
- **R2.3.3 Multi-source strict-wins.** A passage duplicated in an off AND
  a strict directory must not resolve arbitrarily (the current lookup is
  `WHERE c.content_hash = ?1 LIMIT 1` — no path filter [V]): resolve ALL
  matches and apply strict-wins across them. Likewise an entity sourced
  from documents in MULTIPLE directories: if any source document's
  directory is strict, the entity is gated (an off directory shields its
  own documents from CONTRIBUTING gating obligations; it never downgrades
  an entity another directory made strict).
  **Per-source resolution (2026-10-08 clarification):** each source
  walks rungs 2–3 on its own. A source NO `folder_ontology` entry or
  `ontology_default` decides (rung 3 climbs) resolves at the residual
  rung 4 — so under a strict `tier_fact` it counts as a STRICT source
  for strict-wins, not as a neutral one. Only when EVERY source decides
  `off` at rungs 2–3 does the entity SKIP; rung 4 never overrides an
  all-`off` resolution (§2.3 "first hit decides"). The same rule
  applies per endpoint for edges (R2.3.0) and to heal (an all-`off`
  entity is ungated: neither retyped nor queued). Matrix cases:
  every source under an `off` folder + strict `tier_fact` → SKIP;
  one `off` source + one unmatched source + strict `tier_fact` →
  GATE (the unmatched source is strict via rung 4).
- **R2.3.4 No-source entities.** Entities with NO resolvable sources
  resolve via rung 3 (host default), then rung 4 (tier_fact) — i.e.
  climb the ladder from the host default. The gate feeds the resolver
  from the PROPOSAL's source documents for LLM mints
  (`curated_proposal_sources` → `documents.path` → the same
  path-to-mode function heal uses — one resolver, one direction of
  travel); GUI `entities::create_entity` and bundle `ensure_entity`
  have NO proposal (r4-m6) — they START at rung 3.
- **R2.3.5 Stale-hash origin ledger.** An entity minted from an
  off-directory source KEEPS that exemption when the source later stops
  resolving: the mint-time origin ledger records the SOURCE DIRECTORY
  PATH (not the mode — nothing is stamped that could be "revived"), and
  heal resolves from the ledger when live resolution returns empty. A
  fact with a `source_ref` that no longer resolves is REPORT-ONLY, never
  auto-retyped — UNDER THE R2.3.2 SCOPE RULE (r21): this applies when
  the resolved `folder_ontology` map has at least one `off` (or
  degraded/dropped) entry, OR the entity has a ledger row carrying a
  `source_directory` (an off-sourced mint, R2.4.6). With an empty map
  and no such ledger row an unresolved source cannot flip any mode, and
  the ladder decides normally (R2.3.2's empty-map remap case). Matrix
  case: off-directory entity + note edit + `heal --yes` → zero retypes
  (the ledger row keeps it report-only even if the off entry is later
  removed from the map).
- **R2.3.6 Heal interaction with off.** An entity with ANY off-directory
  source is REPORT-or-QUEUE for heal, never auto-retyped — strict-wins
  flips the entity's mode when a strict source appears later, and an
  automatic retype at that moment would destroy a type the user
  deliberately left unchecked (D8). Every heal retype — an alias retype
  OR an approved queue retype — writes an `alias_retype`/`queue_retype`
  origin row (R2.4.6) carrying the pre-retype label, so any eventual
  retype is reversible; under first-origin-wins an entity that already
  has a ledger row keeps it, and its `original_type` is still the
  reversal target. Matrix case: off-minted
  entity later gains a strict source → no automatic retype, heal queues a
  proposal.

### 2.4 Write-time node-type gate

**R2.4.1 Prompt:** the closed declared set is appended to the synthesis
prompt as a clause (same pattern as the edge-vocabulary clause,
`build_system_prompt` `synthesis.rs:625-665`). The `:578/:591` consts stay;
the clause is appended. For NEW entities the set is resolved from
`tier_fact` (new entities have no id pre-commit; the existing edge code
already routes new targets through the shared tier_fact list,
`synthesis.rs:1167-1168, 1181-1187`). The open `person|project|concept|...`
ellipsis invitation is not edited in place — the closed list rides the
appended clause. Drifted types visible in candidate listings
(`format_candidates_section`, `synthesis.rs:549-551`) self-correct once
migration/heal runs.

**R2.4.2 One insert helper.** Create a single shared entity-insert function
housing the `NodeVocabulary::admit` check and move ALL FOUR insert sites
(§1.2) onto it — a future fifth path cannot bypass the gate. The helper
takes an `ImmediateTx<'c>` newtype (r2-M7) that can ONLY be constructed
through `Connection::transaction_with_behavior(TransactionBehavior::
Immediate)` — the redirect check + fallback lookup + insert sequence must
be atomic (read-check-insert races re-create duplicates the merge sweep
just removed), and a plain `&Connection` caller is a compile-time
impossibility. **Lock hold time (r21):** IMMEDIATE takes the write
lock at BEGIN, which rules out SQLite's deferred-upgrade deadlock;
concurrent writers (GUI vs. background import/heal) wait on the
connection's busy timeout (rusqlite's 5 s default, or the explicit
5 s set on the watcher/reconcile/MCP connections) rather than
deadlock. The cost is contention, so the helper's transaction holds
ONLY the redirect check, vocabulary lookup, insert, and ledger row —
no LLM calls, network, or filesystem I/O inside it (synthesis output
is complete before `resolve_proposal` opens its transaction). Bundle
import already holds one IMMEDIATE transaction for the whole bundle
(`bundle_apply.rs:320`); wave 1 adds no longer hold than that. Callers that already hold an IMMEDIATE transaction wrap
it: `commit.rs:2362` and `bundle_apply.rs:320` (both
`transaction_with_behavior(Immediate)`) — wrapping an EXISTING
`Transaction` cannot guarantee the newtype's invariant (rusqlite exposes
no behavior getter), so those callers open the transaction through an
`ImmediateTx::begin(&mut Connection)` constructor instead of relying on
where they got it (r3-m7). Callers that MUST change:
- `entities_api.rs:44` — `create_entity(&guard.0, …)` runs bare
  autocommit; open an IMMEDIATE transaction around it (r1-MAJOR-3).
- `entities_api.rs` mutators — `update_entity_summary` /
  `archive_entity` (`entities.rs:567-592`) take `&Connection` and run
  single autocommit statements; with redirect resolution + the
  archive-the-cluster rule they become read-then-multi-write and MUST
  open IMMEDIATE transactions (r17-m5) — their signatures change to
  `&mut Connection` (through `ImmediateTx::begin`), rippling into the
  `entities_api.rs` callers; do NOT reach for
  `Connection::unchecked_transaction`, which would undermine the
  `ImmediateTx` guarantee (r18-m5).
- `okf_migration` (r2-M7) — `run_okf_migration(conn: &Connection, …)`
  executes raw `BEGIN IMMEDIATE` (`okf_migration.rs:173`) with no
  rusqlite `Transaction`; it needs `&mut Connection` to open a real one
  (propagate the signature change up its call chain), and its entity
  insert is an `ON CONFLICT(id) DO UPDATE` upsert, not a plain insert:
  the shared helper gains an explicit upsert mode — update
  `name`/`summary`/`updated_at` EXACTLY as the current SQL does
  (`okf_migration.rs:96-99`), never touching `entity_type` (r6-M3:
  dropping `name` from the update would strand stale names on a
  post-abort retry) — so the migration keeps semantics without
  bypassing the helper.

**R2.4.3 NodeVocabulary.** Add a node-side counterpart to
`EdgeVocabulary` (edge canonicalization: trim/lowercase/canonicalize,
#189) as the single casing owner: `NodeVocabulary::canonicalize` +
`admit`. `Person` vs `person` must not become new drift.

**R2.4.4 Degrade ladder (Kurt D3 + OQ1 confirmed: degrade, don't kill).**
Undeclared type on admit → canonicalize casing → apply the signed alias
table (§2.6.2) → fall back to the manifest's DECLARED FALLBACK (never a
hard-coded literal). Single consolidated rule:

- Every strict manifest MUST declare its own fallback type; absence is a
  configuration error (§2.4.5).
- If the fallback type is not itself declared → same configuration error.
- **Manifest vocabulary extension (the r2-B1 gap, resolved here).** The
  current manifest JSON carries only `{"node_types": [...],
  "edge_types": [...]}` (`commit.rs:4186`) — no fallback key exists, and
  CT has no production manifest writer (§1.6). Wave 1 therefore defines
  and seeds it:
  - Key: `"fallback_node_type": "<type>"`, an OPTIONAL top-level sibling
    of `node_types`/`edge_types` inside `manifest_json`. Absence is a
    defined TRANSITION state, not a stall: the gate treats a strict
    manifest without the key as "declared set only, no fallback" and
    classifies it per §2.4.5 — BUT wave 1 closes the gap with an
    IDEMPOTENT ENSURE step (r6-B1, superseding the earlier
    one-shot-migration design): `ensure_manifest_vocabulary(conn, entity_id)` runs
    whenever the gate or heal RESOLVES a manifest (memoized per
    entity+generation, where generation = sha256 of `manifest_json`,
    r10-MINOR-3) and — best-effort (r8-m6) — at DB open, invoked BEFORE `run_okf_migration` (the `let _ = run_okf_migration(...)` call in `AppDb::open_with_config`, `connection.rs`) and after `migrate()` (the ensure runs best-effort at DB open in the `AppDb` path — `AppDb::open_with_config` in `connection.rs` (its `migrate()` + `run_okf_migration` block), the ONLY path that also runs the OKF migration, which is the ordering that matters; `migrate_open_db`/`migrate_brain_db` wrappers noted r19-m5;
    resolution-time is the real guarantee) — so fresh installs (whose
    `tier_fact` is
    seeded AFTER migration by `seedManifestsIfAbsent`,
    `src/lib/ontologySeed.ts:47-52`, with `ifAbsent: true`) are covered
    on first gate/heal resolution, and no deployed brain ships without
    a fallback for long. The ensure is a no-op when its work is already
    done. On a READ-ONLY or contended connection the ensure's write
    may fail (non-app hosts open best-effort via `migrate_brain_db`, `connection.rs`, which warns rather than fails on a read-only or contended DB):
    then it computes the ensured vocabulary IN MEMORY for the current
    resolution and the heal report says "ensure pending (read-only)"
    instead of a §2.4.5 error (r12-m4 — never a false loud
    configuration error on a read-only path). **Write-during-report-command is ALLOWED and pinned
    (r10-MINOR-3):** the ensure's `manifest_json` write during
    report-only runs (report heal, `ct wiki sweep`, prompt build) is
    accepted — idempotent, single-statement, and race-safe enough (the
    engine overwrites `manifest_json` whole-row with its own manifest
    anyway; the ensure simply re-runs on the next resolution if
    clobbered). Limiting the ensure to `--yes` runs would leave fresh
    installs uncovered on first resolution. Memo key: `(entity_id,
    sha256(manifest_json))`, recorded ONLY after the write commits
    (r11-m4: an in-transaction memo + rollback would leave the row
    un-ensured but memo-marked until restart — held mints with a false
    §2.4.5 error). **The ensure carries the FULL vocabulary work (r7-MAJOR-2/3):
    it adds `document` + `process` to `node_types` AND writes
    `fallback_node_type` under one seed-set guard — the earlier one-shot-migration
    design is gone: nothing extends manifests at all except the ensure (a fresh install's row
    does not exist at migration time, and the TS engine can rewrite
    `manifest_json` via `setManifest` upsert /
    `mergeManifestUpdates`, engine `chunk-J2TWKOEE.mjs:3175-3206`,
    so a one-shot extension would be silently lost on the first
    engine rewrite). Guard match = SUBSET: the EA seed's 17 type slugs
    ⊆ the row's declared `node_types` (idempotent after extension,
    r7-MAJOR-1); foreign manifests fail the guard and get the
    fallback-only treatment below. ADDED ENTRIES ARE OBJECTS
    (`{"type":"document","description":"…"}`-shaped, matching the
    engine's `OntologyNodeType`) — bare strings FAIL the engine's
    `validateManifest` (`node.type?.trim()` on a string is undefined →
    "slug must be non-empty"; verified live). The engine's
    `validateManifest` tolerates the extra top-level `fallback_node_type`
    key (it reads only `node_types`/`edge_types`; verified live), so the
    TS side does not break; the cross-repo follow-up adds both to the
    seed schema properly.
    `EdgeVocabulary`/edge behavior is untouched (edges have no fallback
    concept).
  - Fallback VALUE rule (r6-M1, universal r11-M3): ONE rule for EVERY
    strict manifest — the ensure picks the first of `concept`, then
    `project`, that the manifest's own `node_types` declares, and
    writes it as `fallback_node_type`; if NEITHER is declared it
    writes NOTHING and reports loudly (heal report + warn) — never a
    type the manifest doesn't declare (that would be a fabricated
    §2.4.5 error on every novel-label mint). EA/`SchemaSoftwareOrg`
    17-type set → `project`; the live ThinkPad row (18 types incl.
    `concept`) → `concept`; SchemaOrg (9 types, declares `project`,
    not `concept`) → `project` — no first-party brain stalls. NOTE — this CORRECTS the frozen investigation
    (§4.A.3 "concept is where the EA manifest happens to declare it"):
    neither shipped SEED declares `concept` (verified live: SchemaOrg =
    person…product, 9 types; SchemaSoftwareOrg = 17 types), but the LIVE
    ThinkPad `tier_fact` row DOES declare it (18 types = seed +
    emergent-merged `concept` [V: re-censused 2026-10-04]), so on THIS
    brain `concept` is legal and `concept` rows are compliant (see the
    `concept` disposition in §2.6.2). The ruling note records this.
    ONE fallback rule, stated once (r9-M3): the ensure prefers
    `concept` WHEN THE ROW DECLARES IT (max generality — matches the
    GUI/OKF `'concept'` pre-fills, `entities.rs:551`,
    `okf_migration.rs:95`, which then admit as declared, no degrade);
    `project` is the fallback ONLY when `concept` is not declared
    (seed-pure brains). The degrade ladder's final rung is always the
    manifest's `fallback_node_type` — never a hard-coded literal — so
    seed-pure brains degrade `concept`-prefill mints to `project`
    through the ladder, visibly, via the ledger. The
    ensure stamps rows passing the seed-set guard with
    `fallback_node_type` per the ONE rule above (`concept` if the row
    declares it — the live ThinkPad row — else `project`) + the
    `document`/`process` entries; other strict manifests get the
    declare-or-report treatment at first resolution.
  - Reader (r14-MAJOR-1 — the parser MUST be extended, else the key
    never reaches the gate): `WikiManifest` (`wiki_graph.rs:65-68`)
    gains `fallback_node_type: Option<String>` with
    `#[serde(default, skip_serializing_if = "Option::is_none")]` and
    `parse_manifest` (`:113-164`) parses it — today every other
    top-level key is DROPPED, so an implementer copying
    `EdgeVocabulary::from_manifest(&WikiManifest)` (`commit.rs:219`)
    would never see the ensure's key and every strict brain would
    read as a false §2.4.5 error. Wire effect (accepted):
    `wiki_get_ontology` results now carry the field.
    `NodeVocabulary::from_manifest` reads it. The ensure EDITS THE
    RAW `manifest_json` Value in place (never parse→add→serialize
    through `WikiManifest` — that drops unknown keys the engine
    stores). Matrix test: ensure → `wiki_get_ontology` →
    `NodeVocabulary` sees the fallback.
  - Reader detail: `NodeVocabulary::from_manifest` parses the key;
    absent-key after ensure has run is the only way to reach §2.4.5
    on a live brain, and it is LOUD (heal report + warn), never silent.
  - Forward path: the TS `createWiki` seed schema gains the key in its
    next release (cross-repo follow-up, NOT a wave-1 blocker — the
    ensure covers existing and fresh brains; RR1-verified: seeds use
    `ifAbsent: true` so re-onboarding/schema switches do NOT overwrite
    an existing row and cannot strip the key);
    `ct ontology set` gains `--fallback <type>` writing the key
    (requires §2.11's writer).
  - Matrix test: pre-wave-1 manifest WITHOUT the key → ensure writes
    the per-manifest fallback, gate functional; manifest seeded AFTER
    the wave-1 migration (fresh install) → ensure covers it on first
    resolution; manifest whose node_types do not include the preferred
    fallback → ensure writes nothing, §2.4.5 loud error on first
    novel-label mint; manifest hand-stripped of the key post-ensure →
    re-ensured on next resolution (idempotent); 18-type row including
    `concept` → ensure writes `concept` as the fallback (r10-M1);
    SchemaOrg 9-type row → `project` (r11-M3).
- **No invented label ever enters `entity_type`.** The pending-review
  insert path is removed by design. Degrades are recorded as UNTYPED in
  the origin ledger (§2.4.6) with the original label; queue items
  reference the ledger, not the column. No reader (recall, injection,
  bundles, outbox, TS consumers) ever sees an invented type. (Skip-path
  writes keep TODAY's landing + a ledger row — §2.5, r2-M2a: a labeled
  GUI/LLM mint lands its label verbatim (no vocabulary exists to
  violate), the `'concept'` literal lands only where it did pre-wave-1
  — blank-type GUI, bundle, `okf_migration` (r22). Pre-existing
  behavior on a no-gate path, not an invention.)

**R2.4.5 Configuration error (SG6, consolidated).** A strict manifest with
zero usable types or no declared fallback is a configuration ERROR:
the gate refuses new-entity mints; the refused proposal is HELD as
pending (its facts are NOT dropped — they re-enter when the manifest
names a fallback); heal reports loudly. No disarm variant exists. Note:
held proposals live in `curated_proposals`, which clear_vault wipes —
they survive until a vault switch on the clear path (per-vault by
design).

**R2.4.6 Type-origin provenance.** Every degrade, every
bundle-imported fallback landing, every SKIP-path landing, and every heal
retype is recorded in the DB — logs-only is invisible to heal. The record
is the `entity_type_origin` LEDGER TABLE (plan Task 0 chose the table
over a column; V26). Heal surfaces origin-tagged rows; the proposal queue
takes the ambiguous ones; bundle-imported fallbacks are explicitly
"untyped", never "typed concept". **Ledger contract (r21 — the V26 DDL as
first landed had `original_type TEXT NOT NULL` and no reason column, so a
label-less landing had no legal value and §2.5's `mode:off` reason had
nowhere to go):**

- Columns: `entity_id TEXT PRIMARY KEY`, `original_type TEXT` (NULLABLE —
  NULL means "no label was supplied", e.g. a bundle import, which carries
  no graph type; never `''` and never an invented sentinel string, which
  a reader could mistake for a real label), `reason TEXT NOT NULL`,
  `source_directory TEXT` (NULL unless the mint's mode came from an `off`
  `folder_ontology` entry, R2.3.5), `recorded_at INTEGER NOT NULL`.
- `reason` is a closed set owned by ONE Rust enum (`OriginReason`; no SQL
  CHECK, so adding a reason never needs a table rebuild — writers go
  through the enum, mirroring `VALID_TIERS`):

  | `reason` | Written when | `original_type` | Heal treatment |
  |---|---|---|---|
  | `degraded` | the gate degrades an undeclared label to `fallback_node_type` (R2.4.4) | the label as proposed (trimmed, pre-canonicalization) | surfaced; queue item references the row |
  | `unlabeled_landing` | bundle import lands a label-less entity as the fallback (§2.5) | NULL | surfaced; queue item references the row |
  | `gate_skipped` | the gate SKIPs and the `'concept'` literal lands (GUI blank-type, bundle, `okf_migration`, §2.5), OR any mint whose mode came from an `off` `folder_ontology` entry (R2.3.5; `source_directory` set) | the label written (the literal `concept` for literal paths; NULL for a label-less bundle entity) | surfaced only once the entity resolves GATE AND its `entity_type` ∉ the declared set |
  | `alias_retype` | heal applies a signed alias (R2.6.2) | the pre-retype label | never surfaced as drift — a reversibility record (R2.3.6) |
  | `queue_retype` | an approved queue item retypes an entity | the pre-retype label | never surfaced as drift — a reversibility record |

- FIRST ORIGIN WINS: writes are `INSERT OR IGNORE` on `entity_id` — the
  ledger records an entity's ORIGIN and later transitions never
  overwrite it. A heal retype of an entity that already has a row adds
  nothing; its `original_type` remains the reversal target, and a
  `source_directory` recorded at mint is never lost (R2.3.5).
- "Recorded as UNTYPED" elsewhere in this spec means a row with reason
  `degraded`, `unlabeled_landing`, or `gate_skipped`.
- V26 revision (unreleased — V26 exists only on this PR's branch and has
  no writer before plan Task 3, so no shipped brain holds a row): the
  `MIGRATION_V26` DDL is edited in place to the shape above. A dev brain
  that already ran the pre-r21 DDL is detected at the V26 apply site
  (`PRAGMA table_info(entity_type_origin)` lacks `reason`): if the table
  is empty it is dropped and re-created; if it is non-empty the open
  fails loudly — never guess a `reason`. Matrix cases: bundle import with
  a declared fallback → `unlabeled_landing` row, `original_type` NULL;
  `heal --yes` alias remap of a pre-wave-1 `agent` row → `alias_retype`
  row with `original_type = 'agent'`; a degraded entity later retyped by
  an approved queue item keeps its `degraded` row; pre-r21 empty table →
  rebuilt on open.

### 2.5 Gate behavior per mint path

- **LLM synthesis path:** type required; gate validates at commit via the
  shared helper; degrade ladder applies.
- **GUI `entities::create_entity`:** no-fallback-exists → refusal error
  shown; otherwise normal ladder.
- **Bundle import (`ensure_entity`):** wave-1 bundles carry NO graph type
  label (export drops entity_type — there is no label to remap from).
  Import lands untyped entities as the MANIFEST'S DECLARED FALLBACK
  (never the literal `concept`). If no fallback is declared, the import
  ABORTS ATOMICALLY with a report, facts intact. If a fallback IS
  declared, the entity + its facts import and the entity goes to the
  review queue — the queue cost is ACCEPTED for wave 1. **When the gate
  SKIPs** (off folder / off host / no manifest, r2-M2a): today's
  `'concept'` literal stands (no vocabulary to violate — SKIP means no
  gate), AND the origin ledger records a `gate_skipped` row
  (R2.4.6; `source_directory` set when an `off` folder caused the
  SKIP), so a later strict flip surfaces these rows to heal as
  ledger-tagged, not as invented types.
- **`okf_migration`:** ids are path-derived, so it resolves the
  `folder_ontology` mode from the note's path like any ingest (NOT a
  no-source path). The abort trigger is per §2.3's ladder OUTCOME
  (r12-M2): abort WITHOUT setting `okf_migrated_at` only when the
  ladder resolves GATE against a strict vocabulary that lacks
  `fallback_node_type` (r3-M1: with no declared fallback there is no
  "declared-fallback slot" to land in — landing `'concept'` would be
  the invented label R2.4.4 forbids; the V7 guard makes retry safe;
  it must NOT defer-and-continue either: a deferred entity is
  stranded forever and its `evt-migrate-*` events,
  `okf_migration.rs:103-110`, would point at an entity never created,
  r1-MAJOR-4 — folded here per r15-m6); on SKIP (no row, off, Emergent/CLI-only brain
  — all normal per §2.1) the migration takes §2.5's skip path
  (`'concept'` + ledger row) and SUCCEEDS, so an Emergent legacy brain
  is never wedged into abort-on-every-open. Config-load failure is
  SCOPED per r3-M2: abort only if a page's path resolution actually
  reaches a dropped key — a degraded-but-irrelevant ingest block does
  not block the migration. **The abort
 must be OBSERVABLE (r6-M2):** the only production caller discards the
 error (the `let _ = run_okf_migration(...)` call in `AppDb::open_with_config`, `connection.rs`) —
 change the caller to log loudly and record a diagnostic (heal report
 surfaces it); test: no-fallback abort → diagnostic visible, retry
 succeeds after the manifest gains a fallback.
- **MCP surface:** the ontology writer is simply NOT registered in
  `tool_dispatch.rs` (compile-time absence, not a runtime gate), with a
  test asserting the MCP catalog lacks it. Gate/validation behavior on
  existing MCP write surfaces follows the shared helper automatically.

### 2.6 Ontology heal

**R2.6.1 Detect:** live entities whose `entity_type` ∉ the resolved
declared set (resolution per §2.3). Drift report first; destructive pass
behind `--yes` (mirrors `ct wiki sweep`). Idempotent; retypes are per-row
BEGIN IMMEDIATE. MERGES ARE NOT A HEAL DUTY (register item 2,
r12-M3): heal never merges — duplicate consolidation lives only in
the standalone `ct wiki merge-duplicates` command (§2.7.1);
R2.6.4's remap-before-merge is an ORDERING rule that command
enforces, not a heal step.

**R2.6.2 Signed alias table (the whole unambiguity rule).** Combined with
Kurt's 2026-10-03 type rulings (msg 1556108687293030442), the complete
wave-1 table and vocabulary action is:

| Drifted type | Disposition | Authority |
|---|---|---|
| `agent` → `role` | alias retype | D1 |
| `component` → `service` | alias retype | D2 |
| `software` → `document` | alias retype | Kurt ruling: software/tools/repos/apps are "documents" |
| `document` | becomes legal: manifest declares `document` | Kurt ruling: written things → `document` |
| `process` | becomes legal: manifest declares `process` | Kurt ruling: procedures/protocols/skills → `process` |

Consequences:
- **Alias-target declaredness (r2-M1):** `NodeVocabulary::admit` and heal
  BOTH verify that an alias TARGET is declared in the resolved manifest
  before retyping; if the target is not declared, the row goes to the
  QUEUE (never retyped onto an undeclared type). Matrix test: alias
  target undeclared → zero retypes, rows queued.
- The alias table ships COMPLETE in wave 1; SG1 (fallback as catch-all)
  is NOT a stop-gap. All drifted `document`/`software`/`process` rows
  can heal (census at investigation freeze: 28 `document` + 2 `process`
  + 6 `software` = 36; [V: live recount 2026-10-04 shows 30 + 2 + 7 =
  39 — drift is growing while ungated, which strengthens the case];
  `document`/`process` become legal when the manifest declares them;
  `software` remaps via the alias).
- **Ordered prerequisite (r2-M1, rewritten r8-M1):** the EA `tier_fact`
  seed gains `document` and `process` as declared types. The seed lives
  in the TS schema package (`createWiki`), which CT cannot write (§1.6)
  — therefore the §2.4.4 idempotent ENSURE performs the ENTIRE manifest
  extension (`document`+`process` entries + `fallback_node_type`) under
  the subset guard, before any gate/heal act: the ensure MUST have run
  on a manifest before node gating or heal consults it (gate/heal call
  it in their resolution path; on a foreign manifest it does only the
  fallback declare-or-report). There is NO one-shot migration step and
  NO abort/disarm story for missing rows (missing row = normal, §2.1).
  Upstream seed sync remains the forward path (same as §2.4.4).
- The gate vocabulary keeps `software` as a manifest-legal type only if
  some entity-level or tier manifest row declares it — no ruling maps
  anything TO it, and the EA seed does not declare it.
- **`concept` disposition (r8-M2):** on brains whose manifest DECLARES
  `concept` (the live ThinkPad row does — emergent-merged), `concept`
  rows are compliant and heal leaves them; the DEGRADE LADDER's
  fallback-to-`concept` remains legal there. On brains whose manifest
  does NOT declare it (both shipped seeds), existing `concept` rows are
  drift with NO alias (Kurt has ruled only agent/component/software/
  document/process) → they go to the QUEUE for a Kurt ruling — never an
  automatic fallback-retype (that would create motion on 33+ rows
  without authority). Named here explicitly, not implicit.
  **WRITE-TIME PARITY (r16-MINOR-1):** on such seed-only brains the
  write gate's automatic `concept`→fallback degrade covers GUI
  blank-type mints (`entities.rs:551`) and OKF-migrated wiki pages
  (`okf_migration.rs:95`) — this asymmetry is RULED ACCEPTABLE
  (new/migrated rows are degraded forward at mint with a ledger row,
  and the degrade is visible to heal via that ledger; heal itself
  never fallback-retypes existing `concept` rows). A seed-only brain
  thus accrues ledger rows, not queue items, per OKF page; the queue
  stays reserved for the 33+ EXISTING drift rows.
- Anything not in the table requires a new Kurt ruling to enter it; the
  table is data (one place), not scattered match arms.

**R2.6.3 Retype mechanics:** in-place UPDATE (ids stable within a host),
logged in the heal summary; ambiguous cases (no alias, type not legalized
by ruling) → proposal queue for Kurt. An off-resolution (per §2.3.6)
means REPORT-or-QUEUE, never silent skip, never auto-retype.

**R2.6.4 Order relative to merge:** the ORDERING rule the standalone
`ct wiki merge-duplicates` command enforces (it runs the signed-alias
remap FIRST — or refuses until heal has — then merges, then queues
only groups still conflicting post-remap (a group of {agent, role} is
one type after the remap, not a conflict). Heal itself never merges
(r12-M3).

### 2.7 Duplicate merge (one-time command; net-new)

**R2.7.1 Shape:** the standalone command `ct wiki merge-duplicates` —
ONE-TIME: NOT SCHEDULED and not a recurring heal duty; it CAN be
re-run manually (survivors get demoted by later Clone imports,
r9-m1). It REFUSES until the signed-alias remap has run for THIS vault (the `alias_remap_completed` marker is DELETED by `clear_vault_tables` — r19-m2: the marker lives in `llm_wiki_meta` which survives the clear, but the rows it vouched for do not; the clear-list test asserts the marker is gone — both landed in plan Task 0: delete at `queries.rs:293`, assertion at `queries.rs:994-1004`) — the
  `alias_remap_completed` marker in `llm_wiki_meta`, set at the end of
  a successful `heal --yes` remap pass (a MARKER, not a live-row
  predicate: report-only rows must not block merging forever,
  r17-m3) — (heal `--yes`
at least once — or it offers to run the remap first), else pre-remap
groups like {agent, role} queue as false type conflicts (R2.6.4).
Report first, destructive pass behind `--yes`. The merge sweep and the heal
census EXCLUDE already-redirected rows (else the sweep re-detects the
live loser by name every run).

**R2.7.2 Matching:** group by normalized name — normalize
parentheses/punctuation variants before matching, or
`Memory Architecture Intent 2026-09-01` vs
`Memory Architecture Intent (2026-09-01)` survive every sweep [V: live
census].

**R2.7.3 Survivor selection — deterministic:** byte-wise/BINARY lowest
entity_id — in BOTH Rust and SQL; NEVER `COLLATE NOCASE` or locale-aware
(`bundle_io.rs:17` uses NOCASE for display order only). `created_at` is
NOT used (import-stamped, second-granular). Accepted bias: `ent_*` sorts
below `entity::*` (0x5F < 0x69), so LLM/GUI-minted entities beat
OKF-migrated path-derived ones on ties — stated, accepted (recorded cost:
two hosts can still diverge when one has an own `ent_*` and the shared
id is `entity::*`; wave-2 transport converges them; revisit if fleet
merges prove common). The merge must be RE-ENTRANT: a survivor can later
become a loser when a peer import brings a new duplicate with a lower id
(imports keep source-host ids). Required test: a survivor demoted by a
later import. **Redirect chains (r2-m6):** resolution is path-compressed
at merge time — when writing loser→B where B is itself a loser, resolve
B's chain first and store loser→final survivor; resolution at read time
is a single hop with a cycle guard (error + report, never an infinite
loop). Required test: 2-hop chain A→B, then B merges into C → A's row
rewrites to A→C; a hand-crafted cycle is detected and reported, not
looped.

**R2.7.4 Type conflicts:** if members of a duplicate group disagree on
entity_type (post-remap), the group goes to the review queue — the
survivor must not silently win with the wrong type.

**R2.7.5 Local redirect (wave-1 semantics — no facts move):**
- **Cluster-closed ontology state (r22).** Because a merge moves no
  rows, a member's `ct_entity_optouts` row, `entity_type_origin` ledger
  row, and pre-merge facts stay keyed to the loser. Rung 1a's opt-out
  lookup, heal's ledger read, and `fact_add`'s Phase-1 dedupe therefore
  cover the whole redirect cluster (a deliberate opt-out on ANY member
  keeps applying — D8), and the `--mode strict` reversal clears opt-outs
  across the cluster. `ct ontology set --entity <loser>` resolves to the
  survivor before writing (and refuses an id naming no entity).
- The loser stays live-but-redirected: a local `merged_into` redirect
  record is written (new table); the loser is NEVER returned as an entity
  by reads.
- **Read-side exclusion is ONE shared predicate, not per-call-site
  patches.** `FROM curated_entities` appears in ~36 queries across ~15
  files (commit ×8, bundle_apply ×5, entities ×4, edge_purge ×3,
  synthesis ×3, wiki_graph, proposals*, tasks, wisdom — full enumeration
  is implementation work; REQUIRED audit step: `rg 'FROM
  curated_entities' src-tauri/src` and cover every hit — the list
  above omits `connections.rs` (×2), `okf_migration.rs` (×2),
  `review_shim.rs`, `proposals_review.rs` (×2), `queries.rs`,
  `bundle_io.rs` (r20-m6)); implement as a `live_entities` VIEW
  (r21 — pinned; the per-reader `NOT EXISTS` alternative is dropped:
  ~36 hand-patched predicates is the error-prone path).
  `live_entities` = `curated_entities` rows with no `entity_redirects`
  row; it does NOT filter `deleted_at` (readers keep their own
  archived-row rules — `get_entity` deliberately returns archived
  detail). Every READER selects from `live_entities`; writers
  (`INSERT`/`UPDATE`/`DELETE`) and the few readers that must see
  redirected rows (redirect resolution itself, the merge sweep,
  `clear_vault_tables`) keep the base table. **Structural guard
  (r21):** a required source-scan test walks `src-tauri/src` and
  `tools/src` and fails on any `FROM curated_entities` / `JOIN
  curated_entities` read that is not on an explicit allowlist (each
  entry names its file and a one-line reason) — a newly added query
  fails CI instead of silently resurrecting losers. The outcome tests
  below stay as the end-to-end check; the scan is what catches a missed
  SELECT. Also add a required test that a merged pair EXPORTS as one
  entity (bundle export `bundle_io.rs:16-17` selects
  `deleted_at IS NULL` rows — an unpatched export ships each loser with
  its facts, and the peer re-imports both with source ids, recreating
  the duplicates cross-host even with "no bundle-format anything").
  **Losers' facts are NEVER dropped (r2-M8):** export maps each fact's
  `entity_id` through the redirect to the survivor at export time — a
  read-side projection, not a wave-2 re-point; excluded-entirely would
  silently lose nine-plus entities' facts from bundle backups and
  transfers (D0). Pinned by test: after a merge, export+re-import on a
  fresh brain yields ONE entity carrying BOTH entities' facts.
- **Survivor→loser edges become self-loops.** After a merge, existing
  edges pointing at the loser resolve to the survivor on read — a
  survivor→survivor self-loop, which the edge writer drops today
  (`same_name_curated_endpoints_are_dropped`, `commit.rs:4475` — that
  drop applies at WRITE time; existing edges' payload/attributes are
  never dropped). Read/graph visibility (r3-m8): a resolved
  survivor→survivor self-loop is SHOWN with its original edge
  attributes (it is real pre-merge provenance), but the merge REPORT
  lists every self-loop produced so the user can prune; auto-hiding is
  explicitly not done (silent data hiding is the #158 failure mode).
  No wave-1 edge rewrites.
- **Read paths must resolve redirects, including TRANSITIVE CLOSURE on
  facts** (r13-m3: `get_entity`'s `EntityDetail.id` MUST be the
  SURVIVOR id, not the requested loser id — `entities.rs:528` echoes
  the argument; assert it in the r11-M4 test, else the GUI pins the
  loser id and later mutations re-hit MAJOR-1): reads for a survivor
  must cover
  `entity_id IN (survivor ∪ redirected losers)` — recall,
  `format_candidates_section` (`synthesis.rs:549`), edge resolution,
  injection, AND the GUI readers (r11-M4): `load_facts`
  (`entities.rs:358-365`), `load_tasks` (`:433-438`), `load_events`
  (`:473-478`), `fact_count`/`open_task_count` in `list_entities`
  (`:270-288`, `:349-350`). `get_entity(loser_id)` (`:504-508`, no
  `deleted_at` filter) REDIRECTS to the survivor (HTTP-style: return
  the survivor's detail; the GUI follows it — a stored link to a loser
  opens the survivor). The redirect is followed REGARDLESS of the
  survivor's `deleted_at` (r21): an archived survivor is returned exactly
  as `get_entity(survivor)` returns it today — archived detail, its
  `deleted_at` populated — never `None`/"not found" (a live-only filter
  on the redirect hop would make a stale loser link look like a deleted
  entity the user never archived). Without this, the loser's facts vanish from recall the
  moment the loser is hidden (nine entities' knowledge disappears —
  fails D0). Required test: after a merge, recall on the survivor returns
  the losers' facts; the survivor's `get_entity` + `list_entities`
  counts include the losers' facts/tasks; `get_entity(loser)`
  returns the survivor (r11-M4).
- **Write/mutate paths must resolve redirects (r13-MAJOR-1):** beyond
  the insert helper (§2.4.2) and fact/edge commit, EVERY entity
  mutator keyed by id resolves a redirected loser id to the survivor
  BEFORE acting — `update_entity_summary` (`entities.rs:567-579`) and
  `archive_entity` (`:582-592`) both filter `id = ? AND deleted_at IS
  NULL` and would otherwise succeed invisibly on a row no reader ever
  shows (silent edit loss). Same for fact/task mutators keyed by
  entity id. EXPLICIT consequence (r15-m3, pinned by test): calling a
  mutator with a loser id acts on the SURVIVOR — so archiving via a
  stale loser link archives the whole cluster (survivor + losers);
  the alternative (rejecting loser ids) was rejected as hostile to
  stale GUI state.
- **Archiving a survivor (r13-MAJOR-1):** `archive_entity(survivor)`
  archives the survivor AND its redirected losers, and bundle export
  (`bundle_io.rs:16`, `deleted_at IS NULL` filter) then consistently
  EXCLUDES the whole archived cluster — loser facts were mapped to the
  survivor at export, so an archived survivor means no facts ship:
  consistent, zero orphans. Matrix case: merge -> archive survivor ->
  export + re-import -> zero orphaned facts, zero entity rows.
  **`entity_redirects` rows are KEPT on archive (r21):** archiving
  touches only `curated_entities.deleted_at`; the cluster's redirect
  rows stay, so a stale loser link still resolves to the (archived)
  survivor and the loser never reappears as a standalone row. There is
  no un-archive path in CT today (`archive_entity` is one-way); if one
  is added it must un-archive the whole cluster through the same
  redirect rows, never the survivor alone. The merge sweep's grouping
  ignores archived clusters (`deleted_at IS NOT NULL` members are not
  candidates), so an archived cluster is never re-merged.
- **Write paths must resolve redirects (restored as its own bullet,
  r15-m4 — it was spliced into the archive bullet):**
  `resolve_proposal` → `create_entity_if_needed` (`commit.rs:2365`),
  `entities::create_entity`, bundle `ensure_entity`, and a candidate
  the model echoes back by id — otherwise new facts/edges attach to a
  loser recall can never see. **Error propagation (r2-m9):** `ensure_entity`'s existence probe today swallows
  DB faults as "not found" (`.ok()`, `bundle_apply.rs:723-729`) — the
  helper's redirect check and existence lookup PROPAGATE errors; a DB
  fault during import aborts the import, never mints a duplicate.
  Required test: commit a fact naming a merged-away loser, assert it
  lands on the survivor.
- **A merge is reversible (r21):** because no facts, edges, or entity
  rows move, deleting a loser's `entity_redirects` row fully restores it
  as a standalone entity — the merge's only state. Required test: merge
  a pair, delete the redirect row, assert the loser reads, recalls, and
  exports exactly as before the merge. Consequently a mistaken
  merge-then-archive is NOT a new permanent loss: the losers were already
  hidden by their redirects, and archive is one-way for EVERY entity
  today (no un-archive path exists — pre-existing, out of wave-1 scope).
  No `unmerge` command ships in wave 1; the merge report lists every
  redirect written so a row can be removed by hand.
- Wave-1 merges emit NO re-points, NO tombstones, NO outbox rows, and no
  bundle-format anything. (The replica converges via the wave-2 outbox
  graph export.)

**R2.7.6 Auto-merge gate:** summaries agree → auto-merge
eligible; else → queue. (Evidence-gated; conservative for a one-time
destructive command.) "Agree" is precise (r8-M3): both summaries
NON-EMPTY and normalized-equal (same normalization as §2.7.2). An
empty-vs-empty or empty-vs-nonempty comparison is NOT agreement — LLM
mints insert `summary = ''` (`commit.rs:1390`), so the largest duplicate
groups would otherwise vacuously "agree" and auto-merge distinct
entities (Adrian ×10 may be several people). Matrix case: two same-name
entities, both empty summaries → queued, never auto-merged.

### 2.8 OKF `entity_type` → `doc_kind` internal rename (SG9; Kurt approved)

- Rename the Rust field and enum (`okf/mod.rs:39`, `:53`) to
  `doc_kind`/`DocKind`, ADDING `#[serde(rename = "entity_type")]` (none
  exists today — the key comes from the field name;
  `rename_all = "snake_case"` at `:34` is a no-op for an already-snake_case
  field). Omitting the attribute breaks parsing of EVERY existing note at
  `parse_frontmatter` (`okf/mod.rs:319-320`).
- **Files on disk are unchanged.** The one wire-visible effect: the MCP
  schema's `$defs` key changes EntityType → DocKind (schemars derives it
  from the Rust type name, `okf/mod.rs:51`); enum values unchanged.
  `#[schemars(rename = "EntityType")]` is REJECTED — keeping the old
  $defs name preserves the exact confusion this kills.
- Add doc comments on the field and enum (they become schemars
  descriptions: "document kind; unrelated to graph node type") — the
  field currently has none; REPLACE the existing enum doc comment at
  `okf/mod.rs:49` ("Entity types for OKF documents"), which reinforces the
  confusion. Workspace pins schemars `1` (root `Cargo.toml:24`) — the
  description sits as a SIBLING of `$ref`; the schema test asserts it in
  whichever shape the pinned version emits.
- The `$defs` exposure exists only in `mcp-server` builds (`cfg_attr`) —
  the schema test MUST run under `--features mcp-server` or it passes
  vacuously.
- Touch points: `Display` impl (`okf/mod.rs:61-72`), render line `:296`,
  every constructor/assert site (enumerate with
  `rg 'EntityType::|OkfFrontmatter' src/okf src/tool_dispatch.rs
  src/lib.rs src-tauri/tests` — ADVISORY only; the integration test
  `src-tauri/tests/mcp_write_integration.rs:13/:21/:115` imports the
  type directly (outside `src/`), and `cargo check` does NOT compile
  tests — the rename gate command is
  `cargo test --all-targets --features mcp-server` (r18-m3); it misses `fm.entity_type` at `:296`, the
  field/enum declarations, and `write.rs:1351`; the compiler is the real
  check).
- Test changes (from the investigation's consolidated table):
  - `test_parse_frontmatter` YAML literal (`entity_type: fact`,
    `okf/mod.rs:629`): keep unchanged — catches a missing serde rename;
    ADD a parallel test asserting a `doc_kind:` key FAILS to parse
    (the field is required, no alias is added — r11-m3 corrected
    rationale: `#[serde(alias)]` affects deserialization only and
    never the emitted key, so it cannot preserve the old wire name;
    and accepting `doc_kind:` on input would bless a vault spelling no
    other tool writes).
  - `EntityType::Fact` asserts (`:637`) and the 9 inline constructors in
    `mod tests` (`:442-:607`) + the `test_fm_with_title` fixture (`:332-343`,
    outside `mod tests`) + `write.rs:1351` + `test_entity_type_display`:
    all are rename sites the compiler will catch (r5-m5/r6-m4). The
    `test_render_frontmatter` constructor at `:607` changes too (it is
    one of the nine); only the render-shape assert at `:618` is
    unaffected (r2-m7).
  - ON-DISK KEY LITERALS ARE NOT RENAMED (r14-m3): the
    `"entity_type"` string in `okf/write.rs` `KNOWN_KEYS` (`:66-75`,
    depended on by `rendered_key_set` `:767-777` and the #231/#245
    guards) and the `"entity_type: "` format string at
    `okf/mod.rs:296` stay EXACTLY as-is — only the Rust
    field/enum names change; a mechanical find-and-replace compiles
    cleanly and then breaks the round-trip/key-drop guards or writes
    `doc_kind:` to disk. Test pins `KNOWN_KEYS` unchanged.
  - NEW serialize round-trip test: `entity_type` key survives
    `serde_json::to_value` (no listed test covers the Serialize/MCP-JSON
    direction today).
  - `test_render_frontmatter` (`:618`): unaffected (guards the render
    literal, not serde).
- The on-disk rename (`doc_kind` key + profile llm-wiki/2 bump + vault
  migration, coordinated upstream) is the registered DEFERRED item —
  NOT blocks-the-gate, out of wave-1 scope.

### 2.9 Data integrity prerequisites

**R2.9.1 Clear-list extension.** Every new table this spec adds (origin
ledger `entity_type_origin`, `merged_into` redirects, queue items) joins the
explicit clear list in `clear_vault_tables`, with the clear-list test
extended. (Landed in plan Task 0 for the three V26 tables: delete batch
`queries.rs:253-255`, test `queries.rs:943`. A queue-item table, if
wave 1 adds one, joins the same list.) Matrix: seeded vault opt-out row survival follows the
decided scoping (directory rows live in config and survive; entity-level
rows do not leak).

**R2.9.2 Split-clear fix (PREREQUISITE of `ct ontology set --entity`).**
Entity-level manifest rows must be cleared or vault-keyed on vault switch
(path-derived entity ids are identical across vaults; `clear_vault_tables`
kept `llm_wiki_entity_manifests` at baseline; the test pinned only
tier-row survival — extend the seed with an entity-level row). Both
switch paths get a test: restore branch and clear branch
(§1.7). (Clear branch landed in plan Task 0: `queries.rs:271-289`,
pinned both directions at `queries.rs:1017-1034`; restore branch
pinned by the test at `db/restore_sync.rs:1047`.)

**R2.9.3 Incidental-off census (heal pre-flight).** Before flipping any
semantics on a brain, heal censuses
`llm_wiki_entity_manifests WHERE mode='off'` and reports; a test pins
that an incidentally seeded off-row keeps the tier_fact gate —
census reports it, nothing migrates it (§3) — via the §2.3 rung-1
unmarked fall-through (§2.3.1's SKIP governs the tier_fact-level case;
only a deliberate opt-out marker skips).

### 2.10 `ct wiki sweep` node-type extension

The sweep (edge types only today) gains node-type drift detection using
the same resolved vocabulary + alias table as heal. Cheap once the
vocabulary check exists; report-only like its edge half unless `--yes`.

### 2.11 The `ct ontology set` CLI

- `ct ontology set --mode off|strict [--entity <id>] [--dir <prefix>]`
  and bare `--mode off|strict` (no `--entity`, no `--dir`) = host-wide
  default written to `ingest.ontology_default` in config (explicitly NOT
  a manifest row).
- `--dir <prefix>` writes to the `ingest.folder_ontology` config map
  (edited through config like folder_tiers).
- `--entity <id> --mode off` writes a `ct_entity_optouts` row
  (deliberate choice, D8; rung 1(a)) — allowed only after R2.9.2
  ships. **Reversal (r13-MAJOR-2):** `--entity <id> --mode strict`
  DELETES the `ct_entity_optouts` row in the same transaction as the
  manifest-row write (else rung 1(a) keeps returning SKIP and X stays
  ungated forever — config edits take effect at the next lookup, §2.1).
  `--mode off` leaves any strict manifest row in place (harmless: rung
  1(a) wins). Matrix test: off → strict → next mint IS gated.
  `--entity <id> --mode strict` writes a strict manifest ROW
  for that entity (mode + the RESOLVED `tier_fact` `node_types` +
  `fallback_node_type` copied VERBATIM — r13-m4: never the raw EA
  seed, the live row has 18 types incl. `concept`; copying the
  17-type seed would make `concept` drift — entity-scoped
  override); the combination with `--dir` is REJECTED (one target per
  invocation). `edge_types` rides along verbatim on `--entity strict`
  (a synthesized empty edge list on a strict entity row would hand the
  edge gate a vocabulary that purges every declared edge).
- `--fallback <type>` writes `fallback_node_type` into the target
  manifest's `manifest_json` (tier/manifest-level, unlike the
  entity-scoped `--entity`); target defaults to `tier_fact`. Declares
  the §2.4.4 key explicitly; the ensure does not run on the flag's
  write path.
- This is the FIRST supported manifest-writer in CT (today every manifest
  INSERT is test-only). The no-manifest case is normative in §2.1 (no
  `tier_fact` row → SKIP for both edges and nodes); the CLI writes a
  warning when operating on a brain with no manifest rows at all.

---

## §3 — Migration and rollout

- Gate + heal + data migration land TOGETHER (Kurt-accepted register
  item 1). The data migration touches `curated_entities.entity_type` only
  (plus the new ledger/redirect tables). **It does NOT extend manifest
  rows** — that is the §2.4.4 idempotent ENSURE's job (r7-MAJOR-2/3: a
  fresh install has no `tier_fact` row at migration time, and the
  engine can rewrite `manifest_json` after a one-shot). A missing
  manifest row at migration time is a NO-OP SUCCESS, not an abort
  (matches §2.1's no-row = SKIP).
- **ROLLBACK HAZARD (r2-M4):** a pre-wave-1 binary's `BrainConfig::write`
  re-serializes `IngestConfig` from its struct and silently deletes
  `folder_ontology`/`ontology_default` (opt-outs gone → everything
  strict on next load), and a pre-fallback-key binary ignores
  `fallback_node_type` in `manifest_json` (harmless — JSON extra key).
  Config loss on downgrade is accepted with restore-from-backup as the
  recovery; documented in the release notes.
- Heal performs the alias remap on first run behind `--yes`.
- (No config migration exists — the Off→off mapping is live resolution
  per R2.2.5/r5-M2, so there is no marker to lose to a restore swap.)
- **Tracking issue (investigation §4.D.6 item 2 / r34-m7):** the
  Sept 8 edge-integrity spec promised a follow-up issue for the
  entity-merge pass and it was never filed (§1.3). THIS spec's
  implementation issue IS that tracking item — the implementer files
  ONE issue for wave 1 and links it from the Sept 8 spec's §6 instead
  of opening a second one. Do not open a duplicate.
- Schema: new column/table for type-origin provenance; new table for
  `merged_into` redirects; opt-out marker representation:
  the `ct_entity_optouts` TABLE (r6-M5/r12-M1 — NOT a manifest-row
  column and NOT a `manifest_json` key; r18-m4 cleanup of the stale
  "marker on manifest rows" phrase), so "off" only ever means
  deliberate opt-out; incidental `off` rows are censused and reported
  by R2.9.3, not moved — no migration step touches rows. **Marker representation (r6-M5, hardened r10-M3):** a CT-OWNED TABLE
  (e.g. `ct_entity_optouts(entity_id TEXT PRIMARY KEY, …)`), NOT a
  column on `llm_wiki_entity_manifests` and NOT a key inside
  `manifest_json`. Verified engine SQL (`index.mjs:3040-3060`,
  `setManifest`): the engine UPSERTS via
  `ON CONFLICT(entity_id) DO UPDATE SET mode=…, manifest_json=…,
  updated_at=…` — an in-place update today, so a column WOULD survive —
  but the engine is versioned independently and `INSERT OR REPLACE` in
  a future release would silently reset any CT column. The separate
  table is immune to engine write shape, joins the R2.9.1/R2.9.2
  clear lists, and the gate/heal lookup is a 2-table read (row present
  in optouts → off marker). Pinned by test: engine manifest rewrite
  (re-seed/merge) does NOT remove an opt-out. The "move
  incidental off rows to absent" step (r8-m5): since unmarked-off rows
  already fall through (§2.3.1), the step is DROPPED — the census just
  REPORTS incidental off rows; only rows WITH the CT marker count as
  opt-outs, and nothing deletes or rewrites rows.
- Restart note (SOP, not code): restart Hermes after any CT upgrade —
  the MCP catalog snapshots at session start.

---

## §4 — Stop-gaps (register per Kurt's §7 rule)

Every "good for now" is flagged. Kurt reviewed this list before
investigation freeze; dispositions below are his.

- **SG1 — fallback as catch-all:** NOT a stop-gap. The alias table ships
  complete (§2.6.2); the declared fallback only catches genuinely novel
  labels.
- **SG2 — bundle tombstones:** wave 2. Wave 1 has NO bundle-format part.
- **SG3 — import re-parent vs redirect:** wave 1 = local redirects
  resolved read+write (§2.7.5); wave 2 = import-applied redirects for
  peers. The id-preserving vs new-id merge choice is wave 2 (wave 1 moves
  no facts).
- **SG4 — per-host-local merges as documented limitation:** REJECTED per
  D7. Wave 1's local redirects are not transport; the outbox graph export
  (wave 2) is the real fix.
- **SG5 — seeded-not-chosen "off" rows:** resolved by the ordered skip
  rule (§2.3) + explicit opt-out marker + census (§2.9.3).
- **SG6 — empty manifest / undeclared fallback:** resolved as a
  configuration ERROR (§2.4.5), not a disarm.
- **SG7 — ct_doctor stale tier model:** standalone upstream fix, not
  deferred by this spec; fold in only if this PR touches CI.
- **SG8 — Hermes MCP catalog staleness:** out of CT scope; SOP carries
  "restart Hermes after CT upgrade".
- **SG9 — OKF rename:** wave 1 = internal rename + serde key (§2.8);
  on-disk rename deferred, coordinated upstream.
- **SG10 — known coverage gaps (recorded, not silently dropped):** TS
  engine typing surfaces; any EA tooling reading entity_type directly.
  Escalate into wave 2 if the fleet test or a consumer breaks on retype.
- **SG11 — bundle format carries graph entity_type:** wave 2, beside
  SG2's tombstones. Wave-1 imports land declared-fallback + queue
  (§2.5), cost accepted.

---

## §5 — Wave 2 boundary (NOT in this spec; listed to pin the split)

Outbox graph export (entities, types, tombstones, edges); bundle format
changes; real re-pointing + loser tombstones; import-applied redirects;
id-preserving vs new-id merge decision; the D.4 seq/producer_id outbox
work and its cross-repo contract with core-llm-wiki (RR4b / RR8 / RR9
open questions from the investigation all gate wave 2, not wave 1).

---

## §6 — Test matrix highlights (minimum bar)

1. Ordered skip rule: one test per rung + the matrix cases named in
   R2.2.2 (full+off same prefix: ingest normally AND skip gating),
   R2.2.4, R2.2.5, §2.3.3–§2.3.6 (corrupt config + heal --yes → zero
   retypes; corrupt config + ingest under formerly-off prefix → proposal
   held; bad folder_tiers value must not erase folder_ontology; bare
   host-off + GUI-minted character → ungated; off-minted entity gains
   strict source → queued, not retyped; stale hash → ledger honored;
   ontology.schema=Off + absent ontology_default → rung 3 resolves
   off LIVE (no one-shot mapping, r5-M2/r6-m2); Emergent → still gated).
1a. No-manifest: fresh brain + one LLM mint → one entity, zero held
   proposals; edge gate disarms as today.
1b. r2 additions: edge endpoints resolving to different modes (gate per
   direction, R2.3.0); unmarked-off `tier_fact` → SKIP + census warning
   (§2.3.1); redirect 2-hop chain compresses to final survivor; cycle
   detected + reported; degraded load + config write → raw ingest keys
   survive byte-for-byte; EACCES config + `heal --yes` → zero retypes;
   alias target undeclared → zero retypes, rows queued; pre-wave-1
   manifest without `fallback_node_type` → ensure writes it; export
   after merge re-imports as ONE entity with both facts' sets.
2. Gate: all four insert paths through the shared helper; `ImmediateTx`
   newtype (plain `&Connection` does not compile; okf_migration upsert
   mode preserves `entity_type`);
   degrade ladder incl. canonicalize/alias/fallback; SG6 refusal + held
   proposal with surviving facts; no invented label ever in
   `entity_type`.
2a. r4 additions: degraded load via `BrainConfig::load()` (not
   `load_lenient`) → dropped-key fields set; healthy config +
   `ct ontology set` survives a write; unreadable entity manifest row →
   nodes/heal report-or-hold, edges fall through; unmarked entity row →
   climbs rungs 2–4.
3. Heal: alias remap (agent→role, component→service, software→document);
   document/process legalized; drift report; idempotency; per-row
   IMMEDIATE retypes.
4. Merge: deterministic BINARY survivor; re-entrancy (survivor demoted by
   later import); punctuation-normalized matching; type-conflict →
   queue; redirect read resolution with transitive fact closure; redirect
   write resolution (fact naming a loser lands on survivor); sweep
   excludes redirected rows; merged pair exports as ONE entity;
   redirected-loser endpoints resolve to survivor on read.
5. Rename: serialize round-trip keeps `entity_type` key; parse of legacy
   key; $defs/DocKind under `--features mcp-server`; description present.
6. Integrity: clear-list covers new tables; entity-level manifest row
   cleared/vault-keyed on BOTH switch paths; incidental-off census;
   restore-branch watermark swap → drift report echoes old watermark,
   `--yes` refuses without explicit confirmation of the echoed old
   hash (r4-m1 — no "confirming config read" exists).
7. MCP: catalog lacks the ontology writer.
8. r9/delta/r16 additions: caller passes None + `vault_path` configured →
   resolves normally; `folder_ontology` non-empty + effective root None
   + `heal --yes` → zero retypes; strict entity row inside an off
   directory → GATED; corrupt `tier_fact` `manifest_json` + one LLM
   mint → held, facts kept + loud diagnostic; unparseable schema →
   `approve_link` → next load still degraded → heal still refused
   (raw_ontology); OntologySelection grid with EXPECTED outcomes (rows:
   SchemaOrg, SchemaSoftwareOrg, Emergent, Off, schema-never-chosen
   (None), schema-unparseable; columns: `ontology_default` absent, off,
   strict — ABSENT column: Off → SKIP (D8-critical: live schema wins
   rung 3), unparseable → HOLD (r16-MAJOR-1), SchemaOrg/
   SchemaSoftwareOrg/Emergent/None → climb to rung 4 (strict manifests
   gate; Emergent/off brains have no row → SKIP); `off` column: SKIP in
   every row (explicit choice wins); `strict` column: GATE in every
   row).
9. r5/r6 additions: stale INLINE evidence hash + off folder + `heal
   --yes`
   → zero retypes (r5-M3); settings schema switch to Off + absent
   `ontology_default` → rung 3 resolves off immediately (r5-M2 live
   rule); provider-init failure rollback → ingest/ontology/vault_path/
   trusted_links/wiki survive (r5-M1/r6-m7); pre-wave-1 ingest block
   loads strictly (r5-m2); manifest seeded AFTER the first ensure run → ensure
   covers it on next resolution (r6-B1); manifest without the preferred fallback → ensure
   writes nothing + loud §2.4.5 (r6-M1); foreign strict manifest →
   ensure applies fallback-only treatment (r6-M1 scope guard); engine rewrite of a
   manifest row → CT opt-out TABLE row survives (r10-M3);
   task/event sources (r5-m7, CORRECTED r11-RR): `llm_wiki_tasks` has
   NO `source_ref` column (its provenance is `okf_sources`, present on
   zero rows on the live brain — verified schema `okf_ddl.rs:72-90` +
   live query) and `llm_wiki_events` has none either (only
   `related_entry_id`). Tasks/events are NOT source-resolution inputs;
   the r5-m7 assumption is dropped. Their entity_id rides the entity's
   own resolution like any other child row; no-fallback okf_migration abort → diagnostic
   visible, retry succeeds (r6-M2); rung-3 IngestPolicy cache carries
   schema/flags (r6-M4); r9 additions: mode=strict via rung 2–3 + no
   vocabulary row → SKIP + warning; vault moved + off prefix + `heal
   --yes` → zero retypes (unplaceable-with-root); V20
   empty-evidence sentinel → report-only (stays 0); plain-path ref
   unresolved → report-only; both-empty-summaries duplicate group →
   queued.
10. r11 additions: empty `folder_ontology` map + stale evidence hash +
   `heal --yes` → alias remaps apply (r11-M2 scope rule); map with an
   `off` entry + stale hash → zero retypes; SchemaOrg 9-type manifest →
   ensure writes `project` fallback (r11-M3); after a merge, survivor's
   `get_entity`/`list_entities` counts include losers' facts and
   `get_entity(loser)` returns the survivor (r11-M4); engine manifest
   rewrite does not remove a `ct_entity_optouts` row (r10-M3); ensure
   memo recorded only post-commit (r11-m4). Tasks/events carry NO
   `source_ref` (schema-verified) — not source-resolution inputs
   (corrects r5-m7).
