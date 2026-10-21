# Confirmation queue: human verification of live facts, recall provenance labels, and the widened activity aperture

**Date:** 2026-10-20
**Status:** Draft
**Branch:** `spec/user-confirmation-queue`
**Priority:** High — activity mode currently delivers nothing (zero human-tier facts in the live brain); this delivery is the path to a populated, trustworthy delivery set.

## Revision history (compressed)

- rev 1 (2026-10-20): initial draft from the converged tier-4 Fable guidance
  memo (run 1, 2026-10-20, $5.65 — vault
  `records/investigations/2026-10-20-confirm-queue-fable-consult/memo-run1.md`)
  and Kurt's rulings below. All `file:line` evidence re-verified against
  `main` @ `3224e3cd` during the consult run.
- rev 2 (2026-10-21): Tier-2 GLM 5.3 review (REQUEST CHANGES; 1 BLOCKER,
  2 MAJOR, 4 MINOR — all adjudicated REAL, all addressed): attestation of
  record pinned to an app-written brain table keyed by content hash (B1
  forgery hole closed); R-A channel corrected — sidecar has no Tauri
  AppHandle, events go via a `.brain/` JSONL drop file the app watches
  (M1); deny = archive + deny-suppression so re-kicks cannot resurrect
  denied facts (M2); Rule 8 amendment now replaces the entire passage
  (m1); NULL/out-of-vocab provenance semantics stated for queue vs
  aperture (m2); added render-leak, deny-suppression, forgery, and
  sidecar-process tests (m3); rate-limit default + fact-id payload (m4).

## 0. Decision record (Kurt rulings — binding; made in Discord thread 1559927525226971258, 2026-10-20/21; logged verbatim in vault `immutable-source-files/agents/memories/confirm-queue-aperture-decision-2026-10-20.md`)

| # | Decision | Ruling |
|---|----------|--------|
| S1 | Aperture widening | `user_confirmed` joins the activity delivery set alongside `user_stated`, with the existing non-shadowing fairness rule |
| S2 | Flag authorship | Intention over verbatim: the agent's faithful restatement of Kurt's directive IS attributable; a "no" on a freshly stated dictum means the agent misunderstood (correction signal) |
| R-A | Active Verification Prompting | An unverified fact matched during recall MUST trigger an active app modal (fact, source quote, why-relevant-now, Confirm/Reject); the agent proceeds with the fact as a lead regardless |
| D1 | Queue membership | **Verification status, not speaker identity.** The queue is the set of unverified facts (~504 day one; UI needs aging/bulk affordances). "Who said it" is display context only |
| D2 | Who may confirm | **App-only confirm.** Agent tools get READ-ONLY queue visibility. Chat "approved" reaches the brain only via the existing proposal-review escalation. An agent-stamped confirm is rejected (the honesty hole the design exists to close) |
| D3 | Ranking | Pure relevance. The fairness rule guarantees pool membership, never re-sorts. `user_stated` and `user_confirmed` compete on the same score |
| D4 | Attribution carrier | **The deposit file.** The marker lives in the deposit note's OKF frontmatter; downstream copies inherit it. Files are the immutable source of truth; the brain is a derived view |
| D5 | App-statement path | **The stated-file pipeline.** App statements are captured as files under `stated/<author>/<year>/` and ingested through the normal librarian pipeline — no delivery-gate exception. This is INTENT Rule 8 as planned |
| D6 | Recall labeling | Provenance + who/when-verified labels added to ALL agent-facing recall tools; `ct wisdom match` CLI output stays byte-identical |
| D7 | Vocabulary | `immutable_document` stays OUT of mid-task delivery for now (fully recallable + labeled; a future widen decision with its own evidence). Confirm also sets `confidence = 'confirmed'` |
| D8 | Spec vehicle | ONE combined spec: this document carries the feature design, the INTENT.md amendment text, and the #282 §5 rev-15 delta; one ladder pass |

## 1. Problem

1. **Chat-stated facts never reach a human tier.** When Kurt states a fact in
   chat and the controller deposits it, the pipeline records the WRITER (an
   agent), not the SPEAKER: facts land `librarian_inferred` even when the note
   quotes Kurt verbatim. The live brain holds 504 active entries, ALL
   `librarian_inferred`, zero `user_stated`/`user_confirmed`.
2. **Human-approved stamps exist but are never exercised on the live brain.**
   `user_confirmed` is already in the provenance vocabulary
   (`wisdom_match.rs:48-53`) and is already emitted — `resolve_proposal`
   chooses it whenever `auto_approve` is false (`commit.rs:2926-2930`; both
   review surfaces pass `auto_approve: false`,
   `proposals_review.rs:161-168`, `:263-265`). The brain shows zero because
   the agents lane auto-approves, stamping `librarian_inferred`
   (`synthesis.rs:1334-1335`, `:1121-1122`).
3. **App-stated facts are delivery-invisible.** The app writes `user_stated`
   rows directly with no evidence and no chunks (`wisdom.rs:143-158`).
   Activity mode runs rule (ii) only, which opens facts through the
   evidence-hash join into `librarian_evidence` (`wisdom_match.rs:935-943`);
   the v1 merge that would catch evidence-less facts is dead in activity mode
   (`wisdom_match.rs:759-761`, `:967`). A fact with no `librarian_evidence`
   row never reaches mid-task delivery regardless of provenance.
4. **Recall serves unverified facts but cannot label them.** Message mode
   passes `provenance_filter = None` (`wisdom_match.rs:762-765`);
   `wiki_context`, `wiki_search` and `curated_recall_context` apply no
   provenance filter (`tool_dispatch.rs:1180-1212`, `:622-658`). But
   `wiki_context` facts expose `tier` not `source_type`
   (`wiki_graph.rs:19-27`) and `curated_recall_context` selects `confidence`
   not `source_type` (`tool_dispatch.rs:437-443`) — a consuming agent cannot
   tell a human-confirmed fact from an inferred one, so it cannot weight it.
5. **Review identity does not land on the fact row.** Approvals record who on
   `curated_proposals.reviewed_by` (2026-09-09 spec §4); the entry INSERT
   writes `source_type` but not `last_verified_by` (`commit.rs:1675-1695`).
   Only draft promotion stamps `last_verified_by`/`last_verified_at` and the
   OKF `verified` list on an entry (`drafts.rs:62-74`) — the shape a queue
   confirm reuses.
6. **The charter does not yet describe this intention**, and does not
   document how user scoping / multi-user verification works at all.

## 2. Approach

Two entry paths feed one queue and one delivery gate. Nothing in the
librarian pipeline changes for Path A — attribution is a property of the
deposit, not of the librarian's output.

- **Path A (chat relay):** Kurt says it to an agent → the agent deposits with
  an attribution marker → the normal kick ingests it → the librarian
  extracts (agent tier, evidence + embedding, unchanged) → the fact is
  recallable immediately (labeled) and sits in the queue → Kurt one-tap
  confirms in the app → `user_confirmed` → activity-delivery eligible.
- **Path B (app capture, "instant confirm"):** Kurt states it in the app →
  the app writes a `stated/<author>/<year>/` file with an app-written
  attestation record (§3.7) → the same kick ingests it → the librarian
  extracts with evidence + embedding → a new third provenance arm stamps
  `user_stated` because every source document has an attestation record
  written by the app. Human tier the moment the librarian finishes; no
  queue step needed.

Rejected alternatives (with reasons):

- **MCP confirm tool** — an agent vouching as Kurt is the honesty hole the
  design closes (D2). The read-only queue list is the only agent surface.
- **Delivery-gate exception for app-written facts** — punches a hole in the
  FP=0 gate story and adds a permanent special case (rejected in favor of
  D5's file path, which IS the charter's own Rule 8).
- **Attribution as a brain-only ledger row or fact column** — cuts against
  INTENT Rule 1's trace-to-file rationale and does not sync with the vault;
  frontmatter is the carrier of truth (D4), with an ingest-time derived
  index for cheap queries (a view over files, recomputable, never
  authoritative).
- **Retiring the direct app write before Path B ships** — breaks existing
  app behavior; instead the direct write stays until Path B lands, and the
  UI labels such facts "recallable, not delivered mid-task" so the gap is
  visible (retired once Path B ships, per D5).

## 3. Design

### 1. Attribution carrier (deposit-file frontmatter; D4)

`render_deposit_file` (`wisdom_deposit.rs:89-104`) gains an optional
frontmatter block. When the caller passes an `attribution` argument —
`{speaker, kind: "verbatim"|"restated_at_direction", source}` (`source` =
Discord message id or equivalent handle) — the note renders:

```markdown
---
attributed_speaker: kurt
attribution_kind: restated_at_direction
attribution_source: discord:1562137256498569278
---
```

Notes without attribution render exactly as today (no frontmatter, byte
identical). `kind` vocabulary: `verbatim` and `restated_at_direction` only —
the agent may attribute only what Kurt said or faithfully restated at his
direction, never its own paraphrase or librarian output (S2; enforced by
argument validation, not by trust). Consequences handled:

- The chunker will see frontmatter lines; the ingest path strips/ignores
  them for evidence quotes.
- At ingest, the frontmatter is parsed into a derived brain-side attribution
  index (keyed by document path, written in the same transaction as
  `deposit_kick_state`, `wisdom_deposit.rs:454-480`). The index is a cache:
  the FILE is authoritative, and a re-ingest repopulates it.

### 2. The confirmation queue (D1)

The queue is a query, not a table. Membership: a fact is in the queue when
it is (a) live (not superseded/expired), (b) its provenance is NOT in
`{user_stated, user_confirmed}` — i.e. `librarian_inferred` and
`immutable_document` facts both appear — and (c) it is not deny-suppressed
(§3). NULL or out-of-vocabulary provenance counts as unverified (in the
queue); the delivery aperture, by contrast, admits only exact vocabulary
values, so NULL/unknown provenance is never delivered mid-task. No
attribution required for membership (D1: verification status is the
criterion). The existing trace
entry → `librarian_evidence` → proposal → `curated_proposal_sources` →
`documents` (the join `deposit_status_row` uses, `wisdom_deposit.rs:183-187`)
supplies each row's evidence quote, so the UI shows the fact beside the
source text it came from. Facts from attributed deposits additionally show
the speaker/attribution metadata (display only). ~504 rows on day one: the
queue UI ships with sort-by-age, bulk-select, and an aging indicator from
the start. Suggested UI word for queue state is **"unconfirmed"** — never
"pending" (Rule 3 reserves pending for un-processed deposits).

### 3. Confirm / deny / edit-then-confirm (D2, D3, D7, app-only)

Confirm runs one UPDATE in the shape of `drafts.rs:62-74`:

- `source_type = 'user_confirmed'`
- `confidence = 'confirmed'` (D7; matches the app's own direct-write
  convention, `wisdom.rs:148`)
- append `{by: "human:kurt", at: <iso>}` to the entry's OKF `verified` list,
  mirrored into `last_verified_by` / `last_verified_at`
- push an outbox Update row (Rule 5 sync) and write an event

The confirm action lives ONLY in the desktop app (which owns the embedding
sweep, `proposals_review.rs:12-22` — app-only confirm gets the sweep for
free). Deny = **archive + suppress**: the fact is archived AND a denial row
is recorded (source document path + normalized fact-text hash + who/when)
so a re-kick of the still-live source document cannot resurrect it as a new
queue row (fact ids are random, `wisdom.rs:138`; there is no content dedupe
at commit). The queue query excludes deny-matched rows; the UI shows a
"denied" section (undoable — un-denying un-archives the fact). The denial
check lives in the QUEUE QUERY only for v1 — the librarian pipeline stays
untouched (a commit-time suppression check would be a librarian change and
is explicitly deferred). Edit-then-confirm is deny + create: for a
recently stated dictum a denial means the agent misunderstood (S2), so the
edit path routes through Path B (write the corrected statement as a
`stated/` file, §7) rather than mutating the agent-tier fact in place —
edit-then-confirm must NOT silently become an agent-independent fact write.
An edit invalidates the embedding (the commit path only trusts a precomputed
vector whose `embed_text` matches stored title/body, `commit.rs:1663-1667`);
the new Path-B fact gets a fresh one via the normal kick.

### 4. Active Verification Prompting (ruling R-A)

When an agent-facing recall tool matches a fact whose provenance is not
human-tier, the sidecar/CLI emits an unverified-wisdom event toward the
desktop app in addition to serving the fact normally. Payload: the fact
id, title/body, the evidence quote / source document reference, the trigger
context (the recall query and the calling surface — the "why relevant
now"). The app renders the modal with Confirm/Reject. Rules:

- **Channel (corrected):** the MCP sidecar is a headless process — it holds
  no Tauri `AppHandle`, and every in-repo Tauri emit requires one
  (`lib.rs:380`, `:576`, `:1679`). So recall emits NOTHING over Tauri.
  Instead the recall tool appends a JSONL event line to a drop file in the
  vault's `.brain/` directory (created on first write); the desktop app
  watches the file and renders the modal. The app owns ack/prune of
  consumed lines. The CLI (`ct` in a terminal) behaves like the sidecar —
  it also cannot open a modal, so it appends to the same drop file.
- Fire-and-forget for the agent: the recall response is never delayed or
  blocked by the modal; the agent proceeds with the fact as a lead
  (labeled, per §5).
- Rate-limited at emit time: the same fact is not re-emitted within a
  session (in-process de-dupe set), and a per-session cap (default 5,
  CLI-configurable) bounds total emits; a broad recall cannot spam modals.
- Headless / app-not-running: the drop file simply accumulates; the app
  drains it on next launch. Never an error; recall never waits.

### 5. Recall provenance labels (D6)

Additive JSON keys only; no existing key changes value:

- `WikiSearchHit` (`wiki_graph.rs:19-27`) gains `provenance: Option<String>`
  (from `provenance_for`, `wisdom_match.rs:264-268`, vocabulary verbatim,
  `None` outside it) and `verified: Option<{by, at}>` (from
  `last_verified_by`/`last_verified_at`). Flows to `wiki_context` and
  `wiki_search`.
- `rank_wiki_entries` (`tool_dispatch.rs:437-443`) adds `source_type`,
  `last_verified_by`, `last_verified_at` to its SELECT and emits the same
  keys. Flows to `curated_recall_context`.
- `ct wisdom match` is UNTOUCHED — it already labels provenance
  (`wisdom_match.rs:623`) and message mode stays byte-identical by
  definition (no `SCHEMA_VERSION` bump, cached-output replay test stays
  green).
- Consumer vocabulary documented in INTENT (glossary): human-tier =
  `user_stated`, `user_confirmed`; agent-tier = `librarian_inferred`;
  `immutable_document` = document-derived, unassigned (D7).

### 6. Activity aperture widening (#282 §5 rev-15 delta)

- `provenance_filter` changes from `Option<&str>` to `Option<&[&str]>`
  (`wisdom_match.rs:524`); the predicate at `:603-607` becomes a membership
  test; the activity arm at `:762-765` passes `&["user_stated",
  "user_confirmed"]`. Filtering stays BEFORE the arm-internal truncate
  (`:632`) — the non-shadowing guarantee is preserved by construction
  (S1/D3: pool membership by provenance, order by score).
- `immutable_document` is NOT admitted (D7). Message mode and the fallback
  path are untouched.
- Calibration argument for §5's precondition (a): the 2026-10-18 activity
  calibration relabeled the 14 kernel facts as `user_stated` in a scratch
  copy (`wisdom_match.rs:111-113`). If the first confirmations are exactly
  those 14, the post-widening aperture set equals the calibrated set and
  (a) is met by construction for the kernel; every later confirmation
  widens the set exactly as a new `user_stated` fact would — no new gate is
  created. The #282 spec's "Out of scope" (`:865-866`) and "Rulings
  carried" (`:883-885`) lines are updated by this delta; §5 bump to rev 15.
- Delivery still requires a non-null `embedding_blob` (eligibility fragment
  `:468-470`) and a stage-1 chunk hit through the evidence join (`:935-943`)
  — which is exactly why Path B exists (§7).

### 7. Path B: `stated/` files and the attested-document provenance arm (D5)

INTENT Rule 8's planned capture, built here:

1. The app writes the statement as a file under
   `stated/<author>/<year>/` with frontmatter recording speaker, capture
   method, and a human-readable attestation note (satisfies Rule 1
   trace-to-file).
2. **Attestation storage (the BLOCKER fix — the predicate input):** the
   attestation of record is a brain-side table
   `stated_attestations(document_path, content_hash, attested_by,
   attested_at)` written ONLY by the app at the moment it creates the
   `stated/` file — the same trust boundary as `last_verified_by` (no
   agent-tool write path exists; the sidecar's connection is read-only).
   The file's frontmatter attestation note is DISPLAY COPY, never the
   predicate input: `resolve_proposal` checks the TABLE (joined by document
   path + content hash recomputed at ingest), so an agent deposit carrying
   attestation-shaped frontmatter cannot forge `user_stated` — the agent
   has no way to insert into `stated_attestations`. A content-hash mismatch
   (file edited after attestation) means NOT attested until the person
   re-attests in the app. INTENT Rule 2's "no attestation record in the
   code yet" status line is closed by this table.
3. The normal kick ingests the file; the librarian extracts facts with
   evidence rows and embeddings — the standard pipeline, unchanged.
4. `resolve_proposal` gains a third provenance arm (`commit.rs:2926-2930`
   is the one decision point): when EVERY source document of the proposal
   has a `stated_attestations` row whose `content_hash` matches the
   ingested file, stamp `user_stated` (not `librarian_inferred`). No queue
   step; the fact is human-tier when the librarian finishes ("instant
   confirm", one LLM generation + one embedding of latency).
5. Until Path B ships, the app's direct `add_wisdom_in_tx` write
   (`wisdom.rs:127-218`) remains, and the UI labels those facts
   "recallable, not delivered mid-task." Once Path B ships, the direct
   write is retired (D5).

### 8. Confirm with no backing fact (Rule-15 lesson)

When Kurt confirms something the librarian did not extract, or types a fresh
statement into the queue view, the app CREATES the fact the only
pipeline-consistent way: write the `stated/` file (Path B), attest, kick.
A direct insert would create an evidence-less fact that never reaches
activity delivery — exactly the gap Rule 2's status line already names.

### 9. INTENT.md amendments (quoted diffs; one PR with the code)

- **Rule 2, body** (after "Attestations are written only by the app, never
  by agent tools."): add — "A fact also earns the human tier when the person
  confirms it in the app's confirmation queue. The confirmation records who
  confirmed it and when on the fact itself and stamps its provenance
  `user_confirmed`. Confirmations, like attestations, are written only by
  the app."
- **Rule 2, status:** reword the closing clause to — "…and the app still
  writes `user_stated` facts directly when the person types them in the
  app; the planned change routes those statements through a `stated/` file
  and the Librarian so they gain evidence and become deliverable
  mid-task."
- **Rule 3:** add — "A fact that has been ingested but not yet confirmed by
  a person is not pending. It is live, recallable, and labeled with its
  provenance; `wisdom_pending` never lists it."
- **Rule 8, body:** replace the ENTIRE passage after the first sentence
  ("Things a person says directly are captured by the app (never by an
  agent) under `stated/<author>/<year>/`, with frontmatter recording who
  said it, how it was captured, and its attestation. They stay below wisdom
  tier until attested.") with — "Statements a person makes directly to the
  app are captured by the app under `stated/<author>/<year>/` with
  frontmatter recording who said it, how it was captured, and its
  attestation; the app records the attestation of record, and such
  statements reach wisdom tier when the Librarian finishes processing
  them. Statements a person makes to an agent may be relayed by that agent
  into its deposit lane with an attribution naming the speaker; such facts
  stay agent tier until the person confirms them in the app. An agent may
  attribute only statements the person said or faithfully restated at the
  person's direction — never the agent's own paraphrase."
- **New Rule 10 — "Unconfirmed facts are recallable, labeled, and never
  delivered mid-task."** Body: "Every live fact is recallable on demand and
  carries its provenance. Mid-task delivery admits only human-tier facts:
  `user_stated` and `user_confirmed`. The confirmation queue is the app's
  list of facts the person has not yet confirmed; confirming stamps the
  fact, it never asks the person to retype. When an unconfirmed fact matches
  during an agent's recall, the app offers the person a confirmation
  prompt." Status: Planned. Why: "a wrong fact pushed mid-task is worse than
  a missed one; a labeled fact on request is neither."
- **New section — "Who verifies: users and scope"** (Kurt's documentation
  duty): CT scopes by VAULT, not by user account. One brain per vault
  (issue #213, implemented); there are no user accounts. Identity is a
  string stamped on the action: proposal approvals record `reviewed_by`
  (`desktop-ui`, the operator's OS account, or `<client>:<os-account>` for
  MCP), fact verifications record `last_verified_by` (by convention prefixed
  `human:`) and `last_verified_at`. Authorship of spoken statements lives in
  file paths and frontmatter (`stated/<author>/<year>/`, deposit
  attribution). All of a person's machines share one brain (Rule 5); a team
  today means separate vaults and separate brains per person — shared or
  cross-person verification is an open decision, deliberately not designed
  here.
- **Glossary:** add "Provenance" (the four values + tier mapping),
  "Attribution" (speaker recorded at deposit), "Confirmation" (a per-fact
  human vouch in the app); footnote "Tier" to separate it from the `tier`
  column (`fact`/`wisdom`/none) used for ranking.
- **"The Active Librarian" planned list:** add — "Provenance from attested
  source documents: a proposal whose every source carries a person's
  attestation lands as `user_stated`. **Planned.**"

### 10. Supersession interplay

Deny-with-correction on an agent-tier fact is archive + create for v1
(§3); it does NOT depend on Rule 4's librarian supersession application
(still unbuilt). The supersession link between the archived fact and the
Path-B replacement can be added when Rule 4 lands.

## 4. Error handling

- **Embedding not yet computed:** a queue item may be actionable before its
  `embedding_blob` exists (the app sweep refills nulls). Confirm does not
  need the vector; the UI shows "not yet searchable" while the blob is null
  so Kurt is not surprised by a confirmed-but-undelivered fact.
- **Headless confirm does not exist** (D2): there is no sweep outside the
  app, which is one more reason confirm is app-only.
- **Concurrent double-confirm:** the UPDATE is idempotent when already
  `user_confirmed` (no-op + same event-once guard as proposal resolution's
  `rows_affected` check, 2026-09-09 spec §3).
- **Deposit-file integrity:** attribution frontmatter rides the existing
  OKF If-Match/updated_at machinery; agents write NEW notes rather than
  editing deposits (vault standing rule), so no staleness path is added.
- **Modal abuse:** per-session rate limit (§3.4); headless drop is silent.

## 5. Testing

- **Aperture unit tests** (`wisdom_match.rs` tests): `user_confirmed` in,
  `immutable_document` out; non-shadowing — a non-admitted fact must never
  displace an admitted one ranked below it; filter-before-truncate pin;
  message-mode `provenance_filter = None` unchanged.
- **Pipeline test:** an attributed deposit's attribution survives
  librarian → proposal → fact and appears on the queue row (full kick on a
  scratch brain).
- **Queue query tests:** membership (live + not human-tier), superseded and
  expired excluded, `immutable_document` included, attribution display
  fields optional.
- **Confirm stamp tests:** the UPDATE shape — provenance, confidence,
  verified list, `last_verified_by/at`, outbox row, event; idempotent
  re-confirm; deny archives; edit-then-confirm produces a Path-B fact, not
  an in-place mutation.
- **Recall label tests:** additive keys on all three tools; JSON schema
  stable otherwise; `ct wisdom match` output byte-identical (cached-output
  replay test must stay green).
- **Attribution render tests:** an unattributed deposit renders byte
  identically to today; frontmatter lines NEVER leak into evidence quotes
  or extracted chunks.
- **Deny-suppression test:** deny a fact, re-kick its still-live source
  document, assert no new queue row for the denied content; un-deny
  restores membership.
- **Path B tests:** attested-source proposal stamps `user_stated`; mixed
  attested/unattested sources stamp `librarian_inferred`; `stated/` ingest
  produces evidence + embedding; attestation-table predicate ignores
  attestation-shaped frontmatter in agent deposits (the B1 forgery test);
  content-hash mismatch after edit → not attested.
- **Active Verification Prompting tests:** event emitted iff an unverified
  fact matches; rate limit; sidecar-process case — the event lands in the
  `.brain/` drop file with no Tauri runtime present; app-not-running
  accumulation and drain on launch; recall latency unaffected.
- **Benchmark:** activity FP stays 0 with the widened aperture on a scratch
  brain seeded with the 14 kernel facts confirmed as `user_confirmed` (the
  by-construction argument in §3.6, made empirical).

## 6. Out of scope (Kurt rulings + inherited)

- Any MCP/CLI confirm tool (D2).
- Admitting `immutable_document` to mid-task delivery (D7 — future decision
  with its own evidence).
- Librarian supersession application (Rule 4) — queue v1 uses archive +
  create.
- Teams / shared cross-person verification (open decision, charter
  documents the gap).
- Question embeddings, ANN, any recall-ranking change (origin never ranks —
  INTENT "Out of scope"; D3 keeps it that way).

## 7. Open questions for Kurt

None. All design decisions are resolved (§0); the plan stage may surface
mechanical choices (exact Tauri event channel, index DDL), which do not
need Kurt unless they contradict this spec.

## Rulings carried

- S1/S2 (step-0), R-A (Active Verification Prompting), D1–D8 — Kurt,
  Discord thread 1559927525226971258, 2026-10-20/21; verbatim log in the
  vault decision note named in §0.
- Inherited: #282 ruling 3 (`user_stated`-only aperture, 2026-10-18) is
  SUPERSEDED by S1 via the rev-15 delta in this spec; #265
  corrections-always-delivered contract still dominates the aperture.
- Ladder note: tier-4 Fable reviewed the underlying investigation as
  guidance run 1 (2026-10-20); the post-convergence Tier-4 spec gate
  (dual-review-cycle "Where Fable runs", placement 1) still runs on THIS
  document after the GLM/Opus ladder converges.
