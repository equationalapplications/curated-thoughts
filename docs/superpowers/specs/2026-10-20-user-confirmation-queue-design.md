# Confirmation queue: human verification of live facts, recall provenance labels, and the widened activity aperture

**Date:** 2026-10-20
**Status:** Implemented 2026-10-22 (PR #294) — confirmation queue, per-hit provenance labels, and the widened activity aperture delivered across Tasks 1-11; see the as-built addendum (rev 4.2) below for landed deltas.
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
  human-tier stamps restricted to app surface or attested human
  instruction; sidecar is NOT read-only — trust boundary restated as
  code-surface + policy, residual risk documented; modal renders from the
  brain by fact id, never from drop-file payload (M1); rate-limit moved to
  drop-file + app side (M2); deny-suppression keyed to document+chunk
  (M3); durability & sync subsection (M4); Path B predicate requires ≥1
  source and all attested, stated/ files ride the deposit lane, human-tier
  arm checked first (M5); Rule 10 text carries the #265 corrections
  exception (M6); confidence write declared with pinning test (M7); file
  paths fixed, attribution parsed at deposit time (single statement),
  SHA-256 content hash pinned, drop-file race notes, Rule 2/8 status
  amendments added, new tests. Plus ruling R-B: verification prompting is
  roadmap-planned for remote transports (Discord); the R-A event is
  transport-agnostic.
- rev 4 (2026-10-21): Tier-4 Fable gate, pass 1 (REQUEST CHANGES; 2
  BLOCKERS, 6 MAJOR, 10 MINOR — all adjudicated REAL, all addressed, mostly
  by DELETION): human-tier stamps are DESKTOP-APP-ONLY — the
  `human_instruction` escape hatch is dropped from v1 (B1: an agent-asserted
  handle is exactly the unverifiable assertion D2 forbids) and CLI
  approvals resolve at agent tier too (M2: the TTY check is hygiene, not
  authentication); chat-"approved" resolves at agent tier and records the
  handle as queue-priority display metadata — the human stamp is one tap in
  the app; confirmations rebuild from an app-written
  `stated/.confirmations.jsonl` mirror, not fact-file frontmatter (B2: the
  vault fact file does not exist and frontmatter-trusted rebuilds violate
  Rules 2/5); the hardening premise corrected — `curated_proposal_decide`
  was REMOVED (INTENT Rule 1), real surfaces are the CLI loop and the
  internal API, and the hardening doubles as a tripwire against
  re-introducing an MCP decide tool (M1); deny-suppression re-keyed to
  (document path, chunk hash, normalized fact-text hash) so sibling facts
  from the same chunk stay confirmable (M3); `rank_wiki_entries` maps
  confidence to an ORDINAL (`confirmed` > `inferred` > unknown) — the
  string compare ranked `confirmed` BELOW `inferred` (M4); per-hit labels
  are `source_type` + `verified` (the `provenance` at `wiki_graph.rs:1953`
  is a top-level trace list, not a per-hit field — M5); the direct app
  write is retired IN THIS PR and the Rule 2 status replaced wholesale
  (M6); minors m1–m10 (vocab cite off-by-one, Rule 8 status "partly
  built", `stated/` folder mode + app-registered kick ledger, hash reuse
  via `hasher::hash_bytes`, `DESKTOP_REVIEWER` constant + server-set
  `reviewed_by`, confirm does not touch `lifecycle_status`, per-drain
  modal cap + snooze preference + read-only-`.brain/` safety, handle
  format validation, `audit_and_seed_kick` named as the index transaction,
  MCP identity format marked historical).
- rev 4.1 (2026-10-21): Tier-4 delta re-gate — **APPROVE WITH NITS** ($3.27;
  review preserved at
  `records/investigations/2026-10-20-confirm-queue-fable-consult/spec294-fable-review2.md`
  in the vault). All four nits folded: N1 — the human-tier predicate is a
  `human_surface: true` flag in `ResolveOptions`, never a `reviewed_by`
  string compare (`USER=desktop-ui` + PTY would spoof it); N2 —
  `approval_instruction_source` dropped (no carrier exists; deposit
  attribution already top-sorts); N3 — confirmation rebuild matches by
  content hash, `fact_id` advisory; N4 — `stated/.*.jsonl` ledgers have
  ingest tier None; JSONL forgery added to residual risk. Gate: no further
  pass needed.

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
| H | Spec hardening (rev 3/4, enforcing D2 — flagged for Kurt at spec approval) | A human-tier provenance stamp is produced ONLY by the desktop app (queue confirm, or Path B attestation at stated-file creation). Every other approval surface — the CLI review loop, the internal Rust API — resolves at agent tier. This closes the pre-existing hole where any `auto_approve: false` approval stamped `user_confirmed`, and acts as a tripwire against re-introducing an agent-callable decide tool |

## 1. Problem

1. **Chat-stated facts never reach a human tier.** When Kurt states a fact in
   chat and the controller deposits it, the pipeline records the WRITER (an
   agent), not the SPEAKER: facts land `librarian_inferred` even when the note
   quotes Kurt verbatim. The live brain holds 504 active entries, ALL
   `librarian_inferred`, zero `user_stated`/`user_confirmed`.
2. **Human-approved stamps exist but are never exercised on the live brain —
   and the surfaces that stamp them are not human-authenticated.**
   `user_confirmed` is in the provenance vocabulary (`wisdom_match.rs:47-52`)
   and is emitted by `resolve_proposal` whenever `auto_approve` is false
   (`src-tauri/src/db/commit.rs:2926-2930`); the review surfaces pass
   `auto_approve: false` (`src-tauri/src/db/proposals_review.rs:161-168`,
   `:263-265`). But the surfaces are the CLI review loop
   (`tools/src/cmds.rs:1722-1731`; its TTY check is documented in-code as
   hygiene, not authentication — a PTY-allocating agent defeats
   `is_terminal()`, `cmds.rs:1723-1726`) and the internal Rust API (a repo
   test approves as `reviewed_by="tessera"` — an agent — and gets
   `user_confirmed`, `proposals_review.rs:633-640`). The 2026-09-09 design
   treats `auto_approve: false` as declaring a human decision; nothing
   enforces it. (An MCP decide tool once existed and was removed — INTENT
   Rule 1: "the four old tools are gone from both the MCP sidecar and `ct`",
   INTENT.md:76-78 — but nothing prevents its reintroduction.)
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

- **Any non-app human-tier stamp** (a re-introduced MCP decide tool, a CLI
  stamp, agent-carried "he approved in chat" handles) — an agent vouching as
  Kurt is the honesty hole the design closes (D2). Every such surface
  resolves at agent tier; the queue sorts agent-resolved approvals carrying
  an instruction handle to the top so the human stamp is one tap in the app
  (§3.7-A). The removed MCP decide tool must stay removed; §3.7-A is also
  the tripwire if one is ever proposed again.
- **Delivery-gate exception for app-written facts** — punches a hole in the
  FP=0 gate story and adds a permanent special case (rejected in favor of
  D5's file path, which IS the charter's own Rule 8).
- **Attribution as a brain-only ledger row or fact column** — cuts against
  INTENT Rule 1's trace-to-file rationale and does not sync with the vault;
  frontmatter is the carrier of truth (D4), with a deposit-time derived
  index for cheap queries (a cache over files, recomputable, never
  authoritative).
- **Vault fact files for confirmations** — rejected at gate (rev 4, B2): no
  fact-file artifact exists today (facts are `llm_wiki_entries` rows; the
  OKF migration purged wiki-tier documents), and a frontmatter-trusted
  rebuild would let any file writer forge `user_confirmed` (Rule 2: files
  without an app-written attestation are agent-written, wherever they sit;
  Rule 5: a machine never raises the trust tier of a fact it received).
  Confirmations rebuild from the app-written `.confirmations.jsonl` mirror
  instead (§3.8).
- **Keeping the direct app write alongside Path B** — Path B ships in this
  PR (§3.7); keeping both would leave two app write paths with different
  delivery semantics. The direct `add_wisdom_in_tx` write is retired in
  this PR (D5, rev 4 M6).

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
argument validation, not by trust). Handle values (`attribution_source`,
and later `approval_instruction_source`, §3.7-A) are validated against
`^[a-z]+:[A-Za-z0-9_.-]+$` at argument validation — they are rendered into
YAML frontmatter and parsed back, so free text is rejected at the door
(rev 4 m8). Consequences handled:

- The chunker will see frontmatter lines; the ingest path strips/ignores
  them for evidence quotes (pinned by test, §5).
- The frontmatter is parsed at DEPOSIT time — inside the
  `audit_and_seed_kick` `with_rw` closure (`wisdom_deposit.rs:454-480`,
  named so the index insert lands in that transaction) — into a derived
  brain-side attribution index keyed by document path. The file is created
  by the same call, so deposit time is the only parse point needed; a
  re-ingest repopulates the index from the file. The index is a cache: the
  FILE is authoritative.

### 2. The confirmation queue (D1)

The queue is a query, not a table. Membership: a fact is in the queue when
it is (a) live (`lifecycle_status` active — not superseded/expired), (b) its
provenance is NOT in `{user_stated, user_confirmed}` — i.e.
`librarian_inferred` and `immutable_document` facts both appear — and (c) it
is not deny-suppressed (§3). NULL or out-of-vocabulary provenance counts as
unverified (in the queue); the delivery aperture, by contrast, admits only
exact vocabulary values, so NULL/unknown provenance is never delivered
mid-task. No attribution required for membership (D1: verification status is
the criterion). The existing trace
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
  convention, `wisdom.rs:148`; ranking consequence handled via an ordinal,
  §3.5)
- append `{by: "human:kurt", at: <iso>}` to the entry's OKF `verified` list,
  mirrored into `last_verified_by` / `last_verified_at`
- push an outbox Update row (Rule 5 sync) and write an event
- append the confirmation to `stated/.confirmations.jsonl` — the durable
  record the rebuild re-derives from (§3.8; there is NO vault fact file)
- `lifecycle_status` is NOT touched (it is already `stable`; the drafts
  UPDATE's `lifecycle_status` write serves a different transition)

The confirm action lives ONLY in the desktop app in v1 (which owns the
embedding sweep, `src-tauri/src/db/proposals_review.rs:12-22` — app-only
confirm gets the sweep for free). R-B makes the modal surface
transport-agnostic so a later Discord transport can carry the SAME
confirm/deny actions (its remote confirm path will need its own
attestation story — designed then, not now; see §6). Deny =
**archive + suppress**: the fact is archived AND a denial row is recorded —
keyed by (source document path + evidence CHUNK hash + normalized
fact-title/body hash, plus who/when): the chunk hash is stable across
re-extraction (chunking is deterministic), and the fact-text hash pins the
SPECIFIC fact, so denying one fact never hides a sibling extracted from the
same chunk (rev 4 M3; pinned by test) while a verbatim re-extraction of the
denied fact stays suppressed. Residual, stated honestly: the librarian
rewords on re-extraction, so a REWORDED re-extraction of a denied fact
re-appears — correct behavior (new wording, new vouch opportunity). The
queue query excludes deny-matched rows; the UI shows a "denied" section
(undoable — un-denying un-archives the fact). The denial check lives in the
QUEUE QUERY only for v1 — the librarian pipeline stays untouched (a
commit-time suppression check would be a librarian change and is explicitly
deferred). Edit-then-confirm is deny + create: for a recently stated dictum
a denial means the agent misunderstood (S2), so the edit path routes through
Path B (write the corrected statement as a `stated/` file, §7) rather than
mutating the agent-tier fact in place — edit-then-confirm must NOT silently
become an agent-independent fact write. An edit invalidates the embedding
(the commit path only trusts a precomputed vector whose `embed_text`
matches stored title/body, `commit.rs:1663-1667`); the new Path-B fact gets
a fresh one via the normal kick.

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
  brain sync/backup. **Emitter safety (rev 4 m7):** drop-file I/O errors in
  the emitter are LOGGED AND SWALLOWED — a read path (`ct wisdom match` and
  every recall tool) must never fail because the prompt drop could not be
  written; a read-only `.brain/` degrades to "no prompts", never an error
  (pinned by test).
- **Transport-agnostic by design (R-B):** the drop file is the LOCAL
  transport of a transport-independent event record. A future Discord
  transport reads the same event stream and renders the same
  confirm/deny prompt remotely; nothing in the recall path knows or cares
  which transport is attached. v1 ships the local modal only (§6).
- Fire-and-forget for the agent: the recall response is never delayed or
  blocked by the modal; the agent proceeds with the fact as a lead
  (labeled, per §5).
- Rate limit lives with the drop file + app, not in the emitting process
  (M2-rev2 — each `ct` hook call is a new process, so in-process de-dupe
  is useless): the emitter appends only if the fact has no UNACKED line in
  the drop file (read-before-append, advisory); the app additionally
  de-dupes, caps modals PER DRAIN CYCLE (default 3 — overflow collapses
  into a queue digest; day-one the queue holds 504 facts, so caps are
  load-bearing from the first session), and honors a "snooze prompts while
  queue > N" preference (rev 4 m7).
- App not running: the drop file accumulates; the app drains it on launch.
  Never an error; recall never waits.

### 5. Recall provenance labels (D6)

Additive JSON keys; no schema key removed or renamed:

- All three agent-facing tools emit per-hit `source_type: Option<String>`
  (vocabulary-filtered via `provenance_for`, `wisdom_match.rs:264-268`,
  `None` outside it) and `verified: Option<{by, at}>` (from
  `last_verified_by`/`last_verified_at`): added to `WikiSearchHit`
  (`wiki_graph.rs:19-27`) for `wiki_context` + `wiki_search`, and to
  `rank_wiki_entries`' SELECT/emit (`tool_dispatch.rs:437-443`) for
  `curated_recall_context`. The top-level `WikiContextResult.provenance`
  trace list (`wiki_graph.rs:1953`) is a different thing (per-call
  fact→source traces) and is UNTOUCHED — `source_type` was chosen over
  `provenance` for the per-hit key precisely to avoid a same-named,
  different-typed homonym (rev 4 M5).
- **Declared, intended behavior change (rev 4 M4):** `rank_wiki_entries`
  currently sorts by `confidence` as a plain string descending
  (`tool_dispatch.rs:480-482`, `:491`) — lexicographically `confirmed` <
  `inferred`, so D7's confidence write would rank confirmed facts DOWN.
  Fix in this PR: confidence is mapped to an ORDINAL (`confirmed` >
  `inferred` > unknown/NULL) before ordering; the §5 pin test asserts the
  ordinal behavior. Note: INTENT "Out of scope" (`INTENT.md:249-251`)
  requires a spec + benchmark for ranking changes; `curated_recall_context`
  has no benchmark, so this change ships with the ordinal pin test as its
  guard and is flagged to Kurt in §7 (test-only exception). The delivery
  aperture's ordering rule is untouched (D3: relevance only within the
  admitted set).
- `ct wisdom match` is UNTOUCHED — it already labels provenance
  (`wisdom_match.rs:623`, `WisdomItem.provenance`) and message mode stays
  byte-identical by definition (no `SCHEMA_VERSION` bump, cached-output
  replay test stays green).
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
   creates the `stated/` file. `content_hash` = the SAME hash routine the
   `documents` table already uses (`hasher::hash_bytes`, SHA-256 — one hash
   function everywhere, so the predicate and the ingest can never
   disagree; rev 4 m4). `resolve_proposal` checks THE TABLE (joined by
   document path + content hash recomputed at ingest), never the file's
   frontmatter — an agent deposit carrying attestation-shaped frontmatter
   cannot forge `user_stated` because no agent-facing tool or CLI command
   writes that table (and `vault_write_note` cannot even place a file under
   `stated/` — it is confined to `immutable-source-files/agents/**` by
   `under_deposit`, `okf/write.rs:483-485`). A content-hash mismatch (file
   edited after attestation) means NOT attested until the person
   re-attests in the app. INTENT Rule 2's "no attestation record in the
   code yet" status line is closed by this table.
   **Trust boundary, stated honestly:** the sidecar process is NOT
   read-only — since the 2026-09-05 write-path work it holds a write
   connection for the wisdom CRUD tools (`mcp_server.rs:351`). Enforcement
   here is CODE-SURFACE + POLICY, not connection-level: no MCP tool, CLI
   command, or internal API exposed to agents inserts into
   `stated_attestations`; only the app's stated-file creation path does. A
   malicious local process could write the database directly — that is
   outside CT's threat model (local-first, single-user machine) and is
   recorded as residual risk rather than defended against.
3. **Folder mode & kick registration (rev 4 m3):** `stated/` is a new vault
   root and gets explicit rows: a folder-rules entry (app-writable,
   agent-non-deposit — the agent deposit allowlist does NOT extend to it)
   in the folder-mode/ingest-tier tables (`connection.rs:770-773` is the
   existing pattern), an entry in the `walk_vault`/`ingest_tier_for`
   logic, and a NON-DEPOSIT kick path: the APP — not an agent kick —
   registers the kick ledger row (`set_kick_state`,
   `wisdom_deposit.rs:474`) when it creates the file. The three
   `stated/.*.jsonl` ledgers (`.confirmations`, `.attestations`,
   `.denials`) have INGEST TIER NONE — they are operational state, never
   documents (rev 4.1, gate N4).
4. The kick ingests the file; the librarian extracts facts with evidence
   rows and embeddings — the standard extraction pipeline, unchanged.
   `stated/` files' proposals list them as source documents.
5. `resolve_proposal` gains a third provenance arm (`commit.rs:2926-2930`
   is the one decision point; human-tier check runs BEFORE the agent-tier
   default): when the proposal has AT LEAST ONE source document and EVERY
   source document has a `stated_attestations` row whose `content_hash`
   matches the ingested file, stamp `user_stated`; otherwise
   `librarian_inferred` (zero sources can never be human-tier). No queue
   step; the fact is human-tier when the librarian finishes ("instant
   confirm", one LLM generation + one embedding of latency).
6. The app's direct `add_wisdom_in_tx` write (`wisdom.rs:127-218`) is
   RETIRED IN THIS PR (rev 4 M6): Path B ships in the same PR, §3.9 routes
   fresh statements through Path B, and keeping both would leave two app
   write paths with different delivery semantics.

### 7-A. Hardening: who may stamp human tier (§0-H; closes the trust hole)

Today ANY `auto_approve: false` approval stamps `user_confirmed` — via the
CLI review loop (`tools/src/cmds.rs:1722-1731`; its TTY check is documented
in-code as "hygiene, not authentication": a PTY-allocating agent defeats
`is_terminal()`, `cmds.rs:1723-1726`) or the internal Rust API (a repo test
approves as `reviewed_by="tessera"` — an agent — and gets `user_confirmed`,
`proposals_review.rs:633-640`; the MCP decide tool once named here is GONE
— INTENT Rule 1 removed it, the dispatcher has only
`curated_proposals_list`, `tool_dispatch.rs:1310`, and the name survives
only in comments). With the aperture widened, any of these becomes a
mid-task delivery vector for non-human-approved content. Fix, enforcing D2
(rev 4: deletions, per the gate):

- **The desktop app is the ONLY human-tier surface.** `resolve_proposal`
  stamps a human tier (`user_confirmed`) ONLY when the decision carries an
  explicit `human_surface: true` FLAG in `ResolveOptions` — set ONLY by the
  desktop app's call sites, never derived from a string (rev 4.1, gate N1:
  the CLI's `reviewed_by` comes from the `USER` env var, so
  `USER=desktop-ui` plus a PTY would satisfy a `reviewed_by ==
  "desktop-ui"` string check and reopen D2; a boolean flag set by app code
  cannot be spoofed by environment). `DESKTOP_REVIEWER`
  (`proposals_api.rs:21`) remains the app's `reviewed_by` LABEL; the flag,
  not the label, is the predicate.
- CLI approvals and internal-API approvals resolve at agent tier
  (`librarian_inferred`) from rev 4 on, writing an event
  `approved_at_agent_tier`. A CLI operator who genuinely approves should
  confirm in the queue — one tap, same as chat-relay facts. (rev 4 M2: the
  TTY check cannot authenticate a human, so it must not confer a human
  stamp.)
- Instruction handles (`approval_instruction_source`) are DROPPED (rev 4.1,
  gate N2: after rev 4 no agent-facing approval surface exists to carry
  one, and nothing verified it anyway). A chat "approved" simply resolves
  at agent tier; queue top-sort for chat-relayed facts comes from the
  deposit's `attribution_source` (§3.1), which already exists. A VERIFIED
  form of instruction-binding returns in the R-B follow-up, when a
  transport can bind the handle to a real human action (rev 4 B1: in v1
  nothing verifies a handle, so it must not stamp).
- The hardening doubles as a TRIPWIRE: if an MCP decide tool is ever
  proposed again, §0-H/§3.7-A is the standing answer — proposal-resolving
  tools stamp agent tier unless the human decision happens in the app.
- The internal Rust API is not an agent surface; the repo test's
  `reviewed_by="tessera"` case is updated to expect agent tier.

### 8. Durability & sync (M4-rev3, B2-rev4)

- **Confirmations:** the app confirm updates the brain row (§3) AND appends
  to `stated/.confirmations.jsonl` —
  `{fact_id, content_hash(title+body), by, at}`, app-written (rev 4 B2:
  there is NO vault fact file to update — facts are `llm_wiki_entries`
  rows; a frontmatter-trusted rebuild would violate Rule 2 and Rule 5's "a
  machine never raises the trust tier of a fact it received"). A brain
  rebuild re-derives `user_confirmed` ONLY from `.confirmations.jsonl`,
  matching by `content_hash` against the rebuilt row's title+body —
  `fact_id` is ADVISORY only (rev 4.1, gate N3: fact ids are random and
  change across rebuilds; the content hash is the stable key); hash
  mismatch → not confirmed (the person re-confirms in the app). Rule 1
  traceability is carried by the evidence source document plus this
  record.
- **Attestations:** `stated_attestations` rows are mirrored at write time
  to `stated/.attestations.jsonl` in the vault (app-written, one JSON line
  per attestation, content-hash keyed). A rebuild repopulates the table
  from the JSONL, re-verifying each file's current hash; mismatched lines
  are dropped (not attested) — the person re-attests in the app.
- **Denials:** mirrored the same way to `stated/.denials.jsonl`
  (document path + chunk hash + fact-text hash + who/when); rebuilds
  repopulate suppression.
- Rule 5's "attestations sync along with the facts they cover" is thus
  true for the new tables: they live in the vault, and the brain copies are
  derived. Outbox rows still sync the brain rows for live machines
  (Rule 5).

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
- **Rule 2, status:** replace the status clause wholesale (rev 4 M6) —
  "**Partly built.** The attestation of record exists for `stated/` files
  (`stated_attestations`, mirrored to `stated/.attestations.jsonl`) and for
  queue confirmations (`.confirmations.jsonl`). The app's direct
  `user_stated` write is retired in this change: statements typed in the
  app are captured as `stated/` files and ingested through the Librarian so
  they gain evidence and become deliverable mid-task. Bulk attestation of
  existing human files and re-ingest of wrongly stamped `user_stated` facts
  are not done; trust for other folders still follows location
  (`deposit_default_tier`)."
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
  "**Partly built.** `stated/` capture, the attestation table and the
  attested-source provenance arm are built with the confirmation-queue
  delivery (2026-10). Project scoping (`works/<project>/`) is not in the
  code." (rev 4 m2 — the rule's first sentence is still unbuilt.)
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
  (`desktop-ui` for the app; for the CLI review loop, the operator's OS
  account via `os_account()` — historical note: an earlier design also
  defined an MCP `<client>:<os-account>` form for a decide tool that has
  since been removed, rev 4 m10), fact verifications record
  `last_verified_by` (by convention prefixed `human:`) and
  `last_verified_at`. Authorship of spoken statements lives in file paths
  and frontmatter (`stated/<author>/<year>/`, deposit attribution). All of
  a person's machines share one brain (Rule 5); a team today means separate
  vaults and separate brains per person — shared or cross-person
  verification is an open decision, deliberately not designed here.
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
  app-not-running accumulation is drained on launch; per-drain modal cap
  (3) + snooze preference bound day-one volume.
- **Residual risk (documented, accepted):** a malicious local process with
  database access can write `stated_attestations` or fact rows directly;
  CT's threat model is local-first single-user, so this is recorded, not
  defended (§3.7). A PTY-allocating agent can defeat the CLI TTY check —
  which is why the CLI confers no human tier at all (§3.7-A). A hand-edited
  or sync-corrupted `stated/.*.jsonl` ledger can at worst suppress facts or
  fail-confirmed rebuilds — a FORGED confirmation line only promotes a fact
  if its content hash matches re-ingested content AND the writer had file
  access, which is the same local-process trust boundary as the database
  itself (rev 4.1, gate N4).

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
  fields optional, deny-suppressed excluded; **sibling test (rev 4 M3):**
  deny fact A; sibling fact B extracted from the same chunk REMAINS in the
  queue.
- **Confirm stamp tests:** the UPDATE shape — provenance, confidence,
  verified list, `last_verified_by/at`, outbox row, event,
  `.confirmations.jsonl` append; `lifecycle_status` untouched; idempotent
  re-confirm; deny archives + suppresses; edit-then-confirm produces a
  Path-B fact, not an in-place mutation.
- **Recall label tests:** additive per-hit `source_type`/`verified` keys on
  all three tools; the top-level `provenance` trace list unchanged;
  `ct wisdom match` output byte-identical (cached-output replay test stays
  green); **ordinal ranking pin (rev 4 M4):** `confirmed` ranks ABOVE
  `inferred` in `curated_recall_context` at equal term overlap (the old
  string compare ranked it below).
- **Attribution render tests:** an unattributed deposit renders byte
  identically to today; frontmatter lines NEVER leak into evidence quotes
  or extracted chunks; handle-format validation rejects free text (m8).
- **Deny-suppression test:** deny a fact, re-kick its still-live source
  document, assert no new queue row for the denied fact's text; un-deny
  restores membership.
- **Path B tests:** attested-source proposal stamps `user_stated`; mixed
  attested/unattested sources stamp `librarian_inferred`; zero-source
  proposal NEVER stamps human tier; `stated/` ingest produces evidence +
  embedding; attestation-table predicate ignores attestation-shaped
  frontmatter in agent deposits (the forgery test); content-hash mismatch
  after edit → not attested; `stated/` folder mode + app-registered kick
  ledger row present (m3).
- **Human-tier hardening tests (§3.7-A):** CLI approval resolves at agent
  tier + `approved_at_agent_tier` event; internal-API approval as an agent
  (`reviewed_by="tessera"`) resolves at agent tier; desktop-surface
  approval (`human_surface: true`) stamps `user_confirmed`; **spoof test
  (rev 4.1 N1):** a CLI-shaped call with `USER=desktop-ui` and
  `reviewed_by="desktop-ui"` but no `human_surface` flag resolves at agent
  tier.
- **Durability tests (§3.8):** brain rebuild re-derives `user_confirmed`
  from `.confirmations.jsonl` with hash re-verification; mismatched
  confirmation entries → not confirmed; `stated_attestations` + denials
  repopulate from vault JSONL; mismatched attestations dropped.
- **Active Verification Prompting tests:** event emitted iff an unverified
  fact matches; drop-file de-dupe + per-drain modal cap (3) + snooze
  preference; sidecar-process case — the event lands in the `.brain/` drop
  file with no Tauri runtime present; modal renders from the brain (payload
  tampering cannot change the displayed fact); app-not-running accumulation
  and drain on launch; **read-only `.brain/` case (rev 4 m7):** the recall
  call succeeds byte-identically with prompt emission disabled; recall
  latency unaffected.
- **Benchmark:** activity FP stays 0 with the widened aperture on a scratch
  brain seeded with the 14 kernel facts confirmed as `user_confirmed` (the
  by-construction argument in §3.6, made empirical).

## 6. Out of scope (Kurt rulings + inherited)

- Any MCP/CLI confirm tool for the queue (D2); CLI and internal-API
  approvals resolve at agent tier (§3.7-A) — the desktop app is the only
  human-tier surface in v1.
- **Remote (Discord) verification transport — PLANNED ROADMAP, not v1
  (R-B):** v1 ships the local modal; the event record and the
  confirm/deny action contract are transport-agnostic so the Discord
  transport is an addition. Its design — including a VERIFIED form of
  instruction-binding, reinstating what §3.7-A defers — is a follow-up
  spec, gated on the same honesty rules.
- Admitting `immutable_document` to mid-task delivery (D7 — future decision
  with its own evidence).
- Librarian supersession application (Rule 4) — queue v1 uses archive +
  create; commit-time deny-suppression (queue-query-level for v1).
- Teams / shared cross-person verification (open decision, charter
  documents the gap).
- Question embeddings, ANN; the ONLY recall-ranking change is the
  confidence ordinal fix (§3.5, declared + pinned; test-only exception to
  the INTENT benchmark rule, flagged in §7). Origin still never ranks
  delivery (D3).

## 7. Open questions for Kurt

None blocking. Four items are flagged for his spec approval: (1) the §0-H
hardening is stricter than first proposed — the desktop app is the ONLY
human-tier surface (enforced by a `human_surface` flag in `ResolveOptions`,
not a string compare); CLI and internal-API approvals resolve at agent tier
(even a human at the terminal confirms via the queue); (2) the confidence
ordinal is a ranking behavior change in `curated_recall_context`, shipped
with a pinning test as a test-only exception to INTENT's spec+benchmark
rule (that tool has no benchmark); (3) R-B is recorded as roadmap (Discord
transport later), with v1 local-only — and the instruction-handle stamp is
deferred to it; (4) the app's direct `user_stated` write is retired in this
same PR. All are consequences of his recorded rulings; none needs a new
decision unless he disagrees.

## Rulings carried

- S1/S2 (step-0), R-A (Active Verification Prompting), R-B (remote
  verification roadmap), D1–D8 — Kurt, Discord thread 1559927525226971258,
  2026-10-20/21; verbatim log in the vault decision note named in §0.
- Inherited: #282 ruling 3 (`user_stated`-only aperture, 2026-10-18) is
  SUPERSEDED by S1 via the rev-15 delta in this spec; #265
  corrections-always-delivered contract still dominates the aperture (and
  is now written into proposed Rule 10).
- Ladder note: tier-4 Fable reviewed the underlying investigation as
  guidance run 1 (2026-10-20) and gated rev 3 (pass 1, REQUEST CHANGES,
  $4.33; review preserved at
  `records/investigations/2026-10-20-confirm-queue-fable-consult/spec294-fable-review-run1.md`
  in the vault); per the gate's closing assessment, a Tier-4 re-gate on the
  rev-4 delta is sufficient — no full ladder repeat.

## As-built addendum (rev 4.2, 2026-10-21)

> Additions-only as-built record carried from the Task-9 review verdict
> (`task9-review-verdict.md`, docs-pass inputs); the normative text above is
> unchanged.

- **`add_entity_fact_cmd` is a capture command as built (input b):** the
  app's direct-write path is retired — the command keeps its name and
  arguments but routes through `capture_statement` (Path B) and returns the
  `stated/` path (`Promise<string>` on the frontend wrapper). Entity
  binding is dropped, with the §9 consequence commented at the site. No
  living doc outside this spec described the old add-fact semantics; this
  note is the record.
- **`stated/` registration as built (input c):** a folder row
  `('stated','synthesize',1)` (app-writable, agent-non-deposit) is seeded
  by the every-open V25 `INSERT OR IGNORE` block — no schema-version bump —
  and the three `stated/.*.jsonl` ledgers (`.confirmations`,
  `.attestations`, `.denials`) are INGEST TIER NONE, excluded by whole
  name at the vault walk. Both are pinned by tests; do not add a migration.
- **Verified-label wording (§5 clarification, as built, E2 ruling):** the
  per-hit `verified` label reads the LATEST `okf_verified` JSON entry,
  gated on `confidence='confirmed'`; all three label writers mirror the
  JSON atomically. This is data-identical to the `last_verified_by` /
  `last_verified_at` columns, and the JSON `at` (ISO 8601) is more
  §3.3-faithful than the column timestamps. §3.5's normative sentence
  stands; this note is the fuller contract.
- **`review_shim.rs` shims (Task-3 verdict C4):** they resolve at AGENT
  tier absent `human_surface: true` and MUST pass the flag explicitly
  before any desk rewiring — fail-closed, zero live callers; corrected
  doc comments in `review_shim.rs`.
