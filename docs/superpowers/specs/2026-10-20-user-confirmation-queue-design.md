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
  record pinned to a brain table keyed by content hash (B1 forgery hole);
  R-A channel corrected — the sidecar has no Tauri `AppHandle`, events go
  via a `.brain/` JSONL drop file (M1); deny = archive + deny-suppression
  (M2); Rule 8 amendment replaces the entire passage (m1); NULL
  provenance semantics (m2); new tests (m3); rate-limit default + fact-id
  payload (m4).
- rev 3 (2026-10-21): Tier-3 Opus review (REQUEST CHANGES; 1 BLOCKER,
  7 MAJOR, 8 MINOR — all adjudicated REAL, all addressed): §0-H hardening —
  `curated_proposal_decide` / CLI review can already stamp `user_confirmed`
  today; human-tier stamps now require an app surface or an attested human
  instruction, else agent tier (B1); sidecar is NOT read-only — trust
  boundary restated as code-surface + policy, residual risk documented;
  modal renders from the brain by fact id, never from drop-file payload
  (M1); rate-limit moved to drop-file + app side (M2); deny-suppression
  re-keyed to (document path + chunk hash) (M3); durability & sync
  subsection — confirm writes the vault fact file AND the brain row,
  app-sealed state mirrored to the vault for rebuild (M4); Path B
  predicate requires ≥1 source and all attested, stated/ files ride the
  deposit lane, human-tier arm checked first (M5); Rule 10 text carries
  the #265 corrections exception (M6); D7 confidence write declared as an
  intended recall-ranking shift with a pinning test (M7); minors: file
  paths fixed, `provenance` key unified with `wiki_graph.rs:1953`,
  attribution parsed at deposit time (single statement), SHA-256 content
  hash pinned, drop-file race notes, Rule 2/8 status amendments added,
  new tests. Plus ruling R-B: verification prompting is roadmap-planned
  for remote transports (Discord); the R-A event is transport-agnostic.

## 0. Decision record (Kurt rulings — binding; made in Discord thread 1559927525226971258, 2026-10-20/21; logged verbatim in vault `immutable-source-files/agents/memories/confirm-queue-aperture-decision-2026-10-20.md`)

| # | Decision | Ruling |
|---|----------|--------|
| S1 | Aperture widening | `user_confirmed` joins the activity delivery set alongside `user_stated`, with the existing non-shadowing fairness rule |
| S2 | Flag authorship | Intention over verbatim: the agent's faithful restatement of Kurt's directive IS attributable; a "no" on a freshly stated dictum means the agent misunderstood (correction signal) |
| R-A | Active Verification Prompting | An unverified fact matched during recall MUST trigger an active app modal (fact, source quote, why-relevant-now, Confirm/Reject); the agent proceeds with the fact as a lead regardless |
| R-B | Remote verification (roadmap) | The verification prompt must LATER extend beyond the desktop app to remote transports — Discord first, because Kurt is more often remote from the laptop. v1 ships the local modal; the design keeps the event transport-agnostic so a Discord transport is an addition, not a redesign |
| D1 | Queue membership | **Verification status, not speaker identity.** The queue is the set of unverified facts (~504 day one; UI needs aging/bulk affordances). "Who said it" is display context only |
| D2 | Who may confirm | **App-only confirm.** Agent tools get READ-ONLY queue visibility. Chat "approved" reaches the brain only via the existing proposal-review escalation. An agent-stamped confirm is rejected (the honesty hole the design exists to close) |
| D3 | Ranking | Pure relevance. The fairness rule guarantees pool membership, never re-sorts. `user_stated` and `user_confirmed` compete on the same score |
| D4 | Attribution carrier | **The deposit file.** The marker lives in the deposit note's OKF frontmatter; downstream copies inherit it. Files are the immutable source of truth; the brain is a derived view |
| D5 | App-statement path | **The stated-file pipeline.** App statements are captured as files under `stated/<author>/<year>/` and ingested through the normal librarian pipeline — no delivery-gate exception. This is INTENT Rule 8 as planned |
| D6 | Recall labeling | Provenance + who/when-verified labels added to ALL agent-facing recall tools; `ct wisdom match` CLI output stays byte-identical |
| D7 | Vocabulary | `immutable_document` stays OUT of mid-task delivery for now (fully recallable + labeled; a future widen decision with its own evidence). Confirm also sets `confidence = 'confirmed'` |
| D8 | Spec vehicle | ONE combined spec: this document carries the feature design, the INTENT.md amendment text, and the #282 §5 rev-15 delta; one ladder pass |
| H | Spec hardening (rev 3, enforcing D2 — flagged for Kurt at spec approval) | A human-tier provenance stamp requires an app-surface decision or an attested human instruction. The existing hole — `curated_proposal_decide` (MCP) and `ct proposals review` stamp `user_confirmed` today, and both are agent-callable — is closed by §3.7-A |

## 1. Problem

1. **Chat-stated facts never reach a human tier.** When Kurt states a fact in
   chat and the controller deposits it, the pipeline records the WRITER (an
   agent), not the SPEAKER: facts land `librarian_inferred` even when the note
   quotes Kurt verbatim. The live brain holds 504 active entries, ALL
   `librarian_inferred`, zero `user_stated`/`user_confirmed`.
2. **Human-approved stamps exist but are never exercised on the live brain —
   and the paths that stamp them are agent-callable.** `user_confirmed` is in
   the provenance vocabulary (`wisdom_match.rs:48-53`) and is emitted by
   `resolve_proposal` whenever `auto_approve` is false
   (`src-tauri/src/db/commit.rs:2926-2930`); both review surfaces pass
   `auto_approve: false` (`src-tauri/src/db/proposals_review.rs:161-168`,
   `:263-265`). But BOTH surfaces are reachable by agents: the MCP tool
   `curated_proposal_decide` (`tool_dispatch.rs:2265`) and the CLI. A repo
   test approves as `reviewed_by="tessera"` — an agent — and gets
   `user_confirmed` (`proposals_review.rs:633-640`). The 2026-09-09 design
   treats `auto_approve: false` as declaring a human decision; nothing
   enforces it.
3. **App-stated facts are delivery-invisible.** The app writes `user_stated`
   rows directly with no evidence and no chunks (`src-tauri/src/db/wisdom.rs:143-158`).
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
   OKF `verified` list on an entry (`src-tauri/src/db/drafts.rs:62-74`) — the
   shape a queue confirm reuses.
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

- **MCP confirm tool for the queue** — an agent vouching as Kurt is the
  honesty hole the design closes (D2). The read-only queue list is the only
  agent surface. The proposal-decide escalation remains for chat-"approved",
  but hardened per §0-H/§3.7-A: without an attested human instruction it can
  no longer produce a human-tier stamp.
- **Delivery-gate exception for app-written facts** — punches a hole in the
  FP=0 gate story and adds a permanent special case (rejected in favor of
  D5's file path, which IS the charter's own Rule 8).
- **Attribution as a brain-only ledger row or fact column** — cuts against
  INTENT Rule 1's trace-to-file rationale and does not sync with the vault;
  frontmatter is the carrier of truth (D4), with a deposit-time derived
  index for cheap queries (a cache over files, recomputable, never
  authoritative).
- **Retiring the direct app write before Path B ships** — breaks existing
  app behavior; instead the direct write stays until Path B lands, and the
  UI labels such facts "recallable, not delivered mid-task" so the gap is
  visible (retired once Path B ships, per D5).

## 3. Design

### 1. Attribution carrier (deposit-file frontmatter; D4)

`render_deposit_file` (`src-tauri/src/wisdom_deposit.rs:89-104`) gains an
optional frontmatter block. When the caller passes an `attribution` argument
— `{speaker, kind: "verbatim"|"restated_at_direction", source}` (`source` =
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
  them for evidence quotes (pinned by test, §5).
- The frontmatter is parsed at DEPOSIT time — in the same transaction that
  registers the kick (`wisdom_deposit.rs:454-480`) — into a derived
  brain-side attribution index keyed by document path. The file is created
  by the same call, so deposit time is the only parse point needed; a
  re-ingest repopulates the index from the file. The index is a cache: the
  FILE is authoritative.

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
`documents` (the join `wisdom_deposit.rs:183-187` uses) supplies each row's
evidence quote, so the UI shows the fact beside the source text it came
from. Facts from attributed deposits additionally show the
speaker/attribution metadata (display only). ~504 rows on day one: the
queue UI ships with sort-by-age, bulk-select, and an aging indicator from
the start. Suggested UI word for queue state is **"unconfirmed"** — never
"pending" (Rule 3 reserves pending for un-processed deposits).

### 3. Confirm / deny / edit-then-confirm (D2, D3, D7, app-only)

Confirm runs one UPDATE in the shape of `drafts.rs:62-74`:

- `source_type = 'user_confirmed'`
- `confidence = 'confirmed'` (D7; matches the app's own direct-write
  convention, `wisdom.rs:148`; ranking consequence declared in §3.5)
- append `{by: "human:kurt", at: <iso>}` to the entry's OKF `verified` list,
  mirrored into `last_verified_by` / `last_verified_at`
- push an outbox Update row (Rule 5 sync) and write an event
- update the vault FACT FILE (frontmatter `source_type` + `verified` list)
  in the same action — the file is the durable record (D4); a brain rebuild
  re-derives the stamp from the file (see §3.8)

The confirm action lives ONLY in the desktop app in v1 (which owns the
embedding sweep, `src-tauri/src/db/proposals_review.rs:12-22` — app-only
confirm gets the sweep for free). R-B makes the modal surface
transport-agnostic so a later Discord transport can carry the SAME
confirm/deny actions (its remote confirm path will need its own
attestation story — designed then, not now; see §6). Deny =
**archive + suppress**: the fact is archived AND a denial row is recorded —
keyed by (source document path + evidence CHUNK hash, not fact-body text:
the librarian rewords on re-extraction, but chunking is deterministic, so
the chunk hash is stable) plus who/when — so a re-kick of the still-live
source document cannot resurrect it as a new queue row (fact ids are
random, `wisdom.rs:138`; there is no content dedupe at commit). The queue
query excludes deny-matched rows; the UI shows a "denied" section
(undoable — un-denying un-archives the fact). The denial check lives in the
QUEUE QUERY only for v1 — the librarian pipeline stays untouched (a
commit-time suppression check would be a librarian change and is explicitly
deferred). Residual: if the source DOCUMENT itself changes, its chunks
change and a genuinely re-extracted fact re-appears — correct behavior (new
content, new vouch opportunity). Edit-then-confirm is deny + create: for a
recently stated dictum a denial means the agent misunderstood (S2), so the
edit path routes through Path B (write the corrected statement as a
`stated/` file, §7) rather than mutating the agent-tier fact in place —
edit-then-confirm must NOT silently become an agent-independent fact write.
An edit invalidates the embedding (the commit path only trusts a
precomputed vector whose `embed_text` matches stored title/body,
`commit.rs:1663-1667`); the new Path-B fact gets a fresh one via the normal
kick.

### 4. Active Verification Prompting (ruling R-A; roadmap R-B)

When an agent-facing recall tool matches a fact whose provenance is not
human-tier, the sidecar/CLI records an unverified-wisdom event toward the
desktop app in addition to serving the fact normally. Payload: the fact
id, title/body, the evidence quote / source document reference, the trigger
context (the recall query and the calling surface — the "why relevant
now"). The app renders the modal with Confirm/Reject. Rules:

- **Channel:** the MCP sidecar is a headless process — it holds no Tauri
  `AppHandle`, and every in-repo Tauri emit requires one (`lib.rs:380`,
  `:576`, `:1679`). So recall emits NOTHING over Tauri. The recall tool
  appends a JSONL event line to a drop file in the vault's `.brain/`
  directory (created on first write); the desktop app watches the file and
  renders the modal. The CLI behaves the same. **Anti-spoof (M1): the modal
  NEVER renders payload text as the fact** — it re-reads the fact, its
  evidence and its queue state from the brain by fact id; the drop-file
  payload supplies only ids and trigger context. A poisoned drop file can
  at worst cause a nuisance modal, never a wrong confirmation. Appends are
  single-line `O_APPEND` writes; the app is the sole pruner (reads, acks,
  truncates); the drop file is ephemeral operational state — excluded from
  brain sync/backup.
- **Transport-agnostic by design (R-B):** the drop file is the LOCAL
  transport of a transport-independent event record. A future Discord
  transport reads the same event stream and renders the same
  confirm/deny prompt remotely; nothing in the recall path knows or cares
  which transport is attached. v1 ships the local modal only (§6).
- Fire-and-forget for the agent: the recall response is never delayed or
  blocked by the modal; the agent proceeds with the fact as a lead
  (labeled, per §5).
- Rate limit lives with the drop file + app, not in the emitting process
  (M2 — each `ct` hook call is a new process, so in-process de-dupe is
  useless): the emitter appends only if the fact has no UNACKED line in the
  drop file (read-before-append, advisory); the app additionally de-dupes
  and caps modal display per drain cycle (default: one modal per fact per
  drain; batched overflow shown as a queue digest).
- App not running: the drop file accumulates; the app drains it on launch.
  Never an error; recall never waits.

### 5. Recall provenance labels (D6)

Additive JSON keys; no schema key removed or renamed:

- `WikiSearchHit` (`wiki_graph.rs:19-27`) gains its provenance via the
  existing `provenance` field already emitted in `wiki_context` output
  (`wiki_graph.rs:1953`) — ONE key, unified, not a duplicate; `wiki_search`
  gains the same. Value from `provenance_for` (`wisdom_match.rs:264-268`,
  vocabulary verbatim, `None` outside it). `verified: Option<{by, at}>`
  (from `last_verified_by`/`last_verified_at`) is new.
- `rank_wiki_entries` (`tool_dispatch.rs:437-443`) adds `source_type`,
  `last_verified_by`, `last_verified_at` to its SELECT and emits the same
  keys. Flows to `curated_recall_context`.
- **Declared, intended value shift (M7):** D7's `confidence = 'confirmed'`
  feeds `curated_recall_context`'s existing confidence-weighted scoring
  (`tool_dispatch.rs:445`, `:482`), so human-confirmed facts rank HIGHER in
  on-demand recall than they did as `librarian_inferred`/low-confidence.
  This is intended — human-trusted facts should rank up when the person
  asks — and it is pinned by a test (§5). It does NOT touch the delivery
  aperture's ordering rule (D3: relevance only within the admitted set) nor
  message mode (`ct wisdom match` byte-identical; cached-output replay test
  stays green).
- Consumer vocabulary documented in INTENT (glossary): human-tier =
  `user_stated`, `user_confirmed`; agent-tier = `librarian_inferred`;
  `immutable_document` = document-derived, unassigned (D7).

### 6. Activity aperture widening (#282 §5 rev-15 delta)

- `provenance_filter` changes from `Option<&str>` to `Option<&[&str]>`
  (`wisdom_match.rs:524`); the predicate at `:603-607` becomes a membership
  test; the activity arm at `:762-765` passes `&["user_stated",
  "user_confirmed"]`. Filtering stays BEFORE the arm-internal truncate
  (`:632`) — the non-shadowing guarantee is preserved by construction
  (S1/D3: pool membership by provenance, order by score). NULL/unknown
  provenance is never admitted (membership test over exact vocabulary
  values).
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
2. **Attestation storage (the predicate input):** the attestation of record
   is a brain-side table `stated_attestations(document_path, content_hash,
   attested_by, attested_at)` written ONLY by the app at the moment it
   creates the `stated/` file. `content_hash` = SHA-256 hex of the file
   bytes at attestation time. `resolve_proposal` checks THE TABLE (joined
   by document path + content hash recomputed at ingest), never the file's
   frontmatter — an agent deposit carrying attestation-shaped frontmatter
   cannot forge `user_stated` because no agent-facing tool or CLI command
   writes that table. A content-hash mismatch (file edited after
   attestation) means NOT attested until the person re-attests in the app.
   INTENT Rule 2's "no attestation record in the code yet" status line is
   closed by this table.
   **Trust boundary, stated honestly (rev 3):** the sidecar process is NOT
   read-only — since the 2026-09-05 write-path work it holds a write
   connection for the wisdom CRUD tools (`mcp_server.rs:351`). Enforcement
   here is CODE-SURFACE + POLICY, not connection-level: no MCP tool, CLI
   command, or internal API exposed to agents inserts into
   `stated_attestations`; only the app's stated-file creation path does. A
   malicious local process could write the database directly — that is
   outside CT's threat model (local-first, single-user machine) and is
   recorded as residual risk rather than defended against.
3. The normal kick ingests the file; the librarian extracts facts with
   evidence rows and embeddings — the standard pipeline, unchanged.
   `stated/` files ride the DEPOSIT lane (they are vault documents; the
   proposals they generate list them as sources).
4. `resolve_proposal` gains a third provenance arm (`commit.rs:2926-2930`
   is the one decision point; human-tier check runs BEFORE the agent-tier
   default): when the proposal has AT LEAST ONE source document and EVERY
   source document has a `stated_attestations` row whose `content_hash`
   matches the ingested file, stamp `user_stated`; otherwise
   `librarian_inferred` (zero sources can never be human-tier — M5). No
   queue step; the fact is human-tier when the librarian finishes
   ("instant confirm", one LLM generation + one embedding of latency).
5. Until Path B ships, the app's direct `add_wisdom_in_tx` write
   (`wisdom.rs:127-218`) remains, and the UI labels those facts
   "recallable, not delivered mid-task." Once Path B ships, the direct
   write is retired (D5).

### 7-A. Hardening: who may stamp human tier (§0-H; closes B1)

Today `curated_proposal_decide` (MCP) and `ct proposals review` stamp
`user_confirmed` on any `auto_approve: false` approval — both are
agent-callable, and a repo test already approves as an agent
(`proposals_review.rs:633-640`). With the aperture widened, that becomes a
mid-task delivery vector for agent-approved content. Fix, enforcing D2:

- `resolve_proposal` stamps a human tier (`user_confirmed`) only when the
  decision carries (a) an app surface (`reviewed_by = "desktop-ui"`), or
  (b) the CLI review loop (operator's OS account — a human at a terminal),
  or (c) an explicit `human_instruction: {source}` argument — an attested
  handle to the human's instruction (e.g. `discord:<message-id>`), the same
  pattern and honesty rules as deposit attribution (S2/D4): the agent may
  pass it only when the human literally instructed the approval.
- Without (a)/(b)/(c), an approval still resolves the proposal (existing
  workflow unchanged) but stamps `librarian_inferred` and writes an event
  `approved_without_human_instruction` — visible, honest, not human-tier.
- The internal Rust API is not an agent surface; the repo test's
  `reviewed_by="tessera"` case is updated to expect agent-tier without
  `human_instruction`.

### 8. Durability & sync (M4)

- **Confirmations:** the app confirm writes the vault fact file
  (frontmatter `source_type: user_confirmed` + `verified` list) AND the
  brain row (§3). The file is the durable record (D4); the outbox row syncs
  the brain (Rule 5). A brain rebuild re-derives the stamp by re-ingesting
  fact files.
- **Attestations:** `stated_attestations` rows are mirrored at write time
  to `stated/.attestations.jsonl` in the vault (app-written, one JSON line
  per attestation, content-hash keyed). A rebuild repopulates the table
  from the JSONL, re-verifying each file's current hash; mismatched lines
  are dropped (not attested) — the person re-attests in the app.
- **Denials:** mirrored the same way to `stated/.denials.jsonl`
  (document path + chunk hash + who/when); rebuilds repopulate suppression.
- Rule 5's "attestations sync along with the facts they cover" is thus
  true for the new tables: they live in the vault, and the brain copies are
  derived.

### 9. Confirm with no backing fact (Rule-15 lesson)

When Kurt confirms something the librarian did not extract, or types a fresh
statement into the queue view, the app CREATES the fact the only
pipeline-consistent way: write the `stated/` file (Path B), attest, kick.
A direct insert would create an evidence-less fact that never reaches
activity delivery — exactly the gap Rule 2's status line already names.

### 10. INTENT.md amendments (quoted diffs; one PR with the code)

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
- **Rule 8, status:** replace "None of this is in the code yet." with —
  "Built with the confirmation-queue delivery (2026-10): `stated/` capture,
  the attestation table, and the attested-source provenance arm."
- **New Rule 10 — "Unconfirmed facts are recallable, labeled, and rarely
  delivered mid-task."** Body: "Every live fact is recallable on demand and
  carries its provenance. Mid-task delivery admits only human-tier facts:
  `user_stated` and `user_confirmed` — with the standing #265 exception:
  corrections of any provenance always flow. The confirmation queue is the
  app's list of facts the person has not yet confirmed; confirming stamps
  the fact, it never asks the person to retype. When an unconfirmed fact
  matches during an agent's recall, the app offers the person a
  confirmation prompt." Status: Planned. Why: "a wrong fact pushed mid-task
  is worse than a missed one; a labeled fact on request is neither."
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
  human vouch in the app), "Attestation of record" (the
  `stated_attestations` row, mirrored to the vault); footnote "Tier" to
  separate it from the `tier` column (`fact`/`wisdom`/none) used for
  ranking.
- **"The Active Librarian" planned list:** add — "Provenance from attested
  source documents: a proposal whose every source carries an attestation of
  record lands as `user_stated`. **Planned.**"

### 11. Supersession interplay

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
  app, which is one more reason confirm is app-only in v1.
- **Concurrent double-confirm:** the UPDATE is idempotent when already
  `user_confirmed` (no-op + same event-once guard as proposal resolution's
  `rows_affected` check, 2026-09-09 spec §3).
- **Deposit-file integrity:** attribution frontmatter rides the existing
  OKF If-Match/updated_at machinery; agents write NEW notes rather than
  editing deposits (vault standing rule), so no staleness path is added.
- **Modal abuse:** rate limits live at the drop file + app (§3.4); the
  modal re-reads the brain so payload poisoning cannot mislead (§3.4);
  app-not-running accumulation is drained on launch.
- **Residual risk (documented, accepted):** a malicious local process with
  database access can write `stated_attestations` or fact rows directly;
  CT's threat model is local-first single-user, so this is recorded, not
  defended (§3.7).

## 5. Testing

- **Aperture unit tests** (`wisdom_match.rs` tests): `user_confirmed` in,
  `immutable_document` out; non-shadowing — a non-admitted fact must never
  displace an admitted one ranked below it; filter-before-truncate pin;
  message-mode `provenance_filter = None` unchanged; NULL provenance never
  admitted.
- **Pipeline test:** an attributed deposit's attribution survives
  librarian → proposal → fact and appears on the queue row (full kick on a
  scratch brain).
- **Queue query tests:** membership (live + not human-tier), superseded and
  expired excluded, `immutable_document` included, attribution display
  fields optional, deny-suppressed excluded.
- **Confirm stamp tests:** the UPDATE shape — provenance, confidence,
  verified list, `last_verified_by/at`, outbox row, event, FACT-FILE
  update; idempotent re-confirm; deny archives + suppresses;
  edit-then-confirm produces a Path-B fact, not an in-place mutation.
- **Recall label tests:** additive keys on all three tools with the
  `provenance` key unified (no duplicate key at `wiki_graph.rs:1953`);
  `ct wisdom match` output byte-identical (cached-output replay test stays
  green); **ranking-shift pin (M7):** a fact whose confidence moves to
  `confirmed` rises in `curated_recall_context` ordering — asserted as
  intended behavior.
- **Attribution render tests:** an unattributed deposit renders byte
  identically to today; frontmatter lines NEVER leak into evidence quotes
  or extracted chunks.
- **Deny-suppression test:** deny a fact, re-kick its still-live source
  document, assert no new queue row for the denied content; un-deny
  restores membership.
- **Path B tests:** attested-source proposal stamps `user_stated`; mixed
  attested/unattested sources stamp `librarian_inferred`; zero-source
  proposal NEVER stamps human tier; `stated/` ingest produces evidence +
  embedding; attestation-table predicate ignores attestation-shaped
  frontmatter in agent deposits (the forgery test); content-hash mismatch
  after edit → not attested.
- **Human-tier hardening tests (§3.7-A):** MCP decide without
  `human_instruction` resolves the proposal but stamps
  `librarian_inferred` + writes `approved_without_human_instruction`; with
  `human_instruction: {source}` it stamps `user_confirmed`; CLI and
  desktop surfaces stamp as today.
- **Durability tests (§3.8):** brain rebuild re-derives confirmed stamps
  from fact files; `stated_attestations` + denials repopulate from vault
  JSONL with hash re-verification; mismatched attestations dropped.
- **Active Verification Prompting tests:** event emitted iff an unverified
  fact matches; drop-file de-dupe + app-side cap; sidecar-process case —
  the event lands in the `.brain/` drop file with no Tauri runtime
  present; modal renders from the brain (payload tampering cannot change
  the displayed fact); app-not-running accumulation and drain on launch;
  recall latency unaffected.
- **Benchmark:** activity FP stays 0 with the widened aperture on a scratch
  brain seeded with the 14 kernel facts confirmed as `user_confirmed` (the
  by-construction argument in §3.6, made empirical).

## 6. Out of scope (Kurt rulings + inherited)

- Any MCP/CLI confirm tool for the queue (D2); MCP decide keeps resolving
  proposals but can no longer produce a human-tier stamp without an
  attested human instruction (§3.7-A).
- **Remote (Discord) verification transport — PLANNED ROADMAP, not v1
  (R-B):** v1 ships the local modal; the event record and the
  confirm/deny action contract are transport-agnostic so the Discord
  transport is an addition. Its design (including how a remote confirm
  attests the human) is a follow-up spec, gated on the same honesty rules.
- Admitting `immutable_document` to mid-task delivery (D7 — future decision
  with its own evidence).
- Librarian supersession application (Rule 4) — queue v1 uses archive +
  create; commit-time deny-suppression (queue-query-level for v1).
- Teams / shared cross-person verification (open decision, charter
  documents the gap).
- Question embeddings, ANN, any recall-ranking change beyond the declared
  confidence shift (origin never ranks — INTENT "Out of scope"; D3 keeps
  delivery ordering pure-relevance).

## 7. Open questions for Kurt

None blocking. Three items are flagged for his spec approval: (1) the §0-H
hardening changes existing behavior — MCP/CLI approvals without an attested
human instruction no longer stamp `user_confirmed`; (2) §3.5 declares the
intended recall-ranking shift caused by D7's confidence write; (3) R-B is
recorded as roadmap (Discord transport later), with v1 local-only. All are
direct consequences of his recorded rulings; none needs a new decision
unless he disagrees.

## Rulings carried

- S1/S2 (step-0), R-A (Active Verification Prompting), R-B (remote
  verification roadmap), D1–D8 — Kurt, Discord thread 1559927525226971258,
  2026-10-20/21; verbatim log in the vault decision note named in §0.
- Inherited: #282 ruling 3 (`user_stated`-only aperture, 2026-10-18) is
  SUPERSEDED by S1 via the rev-15 delta in this spec; #265
  corrections-always-delivered contract still dominates the aperture (and
  is now written into proposed Rule 10).
- Ladder note: tier-4 Fable reviewed the underlying investigation as
  guidance run 1 (2026-10-20); the post-convergence Tier-4 spec gate
  (dual-review-cycle "Where Fable runs", placement 1) still runs on THIS
  document after the GLM/Opus ladder converges.
