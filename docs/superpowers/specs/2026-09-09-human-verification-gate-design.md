# Human verification gate for librarian synthesis (proposal review loop)

**Date:** 2026-09-09
**Status:** Implemented 2026-09-10 (PR #201) — supersedes Draft rev 2 (GLM 5.3 frontier review folded: I-1–I-4, M-1–M-6, T-1–T-4)
**Branch:** spec/human-verification-gate
**Priority:** P1

## Problem

Librarian-synthesized facts enter brain.db with no human (or agent-under-rules)
verification. The #186 Phase-2 flip (evidence gate, spec
`2026-09-09-issue186-phase2-flip-design.md`) blocks facts with no chunk
anchoring, but a fact can be perfectly anchored and still be a wrong or
worthless conclusion. Provenance is machine-checkable; judgment is not. Today
the only reviewer is nobody: `folder_rules.auto_approve = 1` for the agents/
tree auto-commits every synthesis proposal.

**Verified current state (live brain, Sep 9):**
- Proposal state machine EXISTS: `curated_proposals.status ∈ {pending,
  approved, partial, rejected, superseded}` (schema CHECK, okf_ddl.rs:167) —
  731 approved / 75 partial / **35 pending** / 34 rejected live rows.
- Per-folder gating EXISTS: `folder_rules(folder_path, librarian_mode,
  auto_approve)`; the single live rule is
  `…/equational-wiki/immutable-source-files/agents|synthesize|1`.
- Synthesis honors it: `librarian/synthesis.rs:1228` — `if auto_approve {
  auto_approve_proposal } else { write_synthesized_event }`.
- Approve path EXISTS and is commit-consistent: `cmds::approve_one_on`
  (tools/src/cmds.rs:415; cli_common hosts list/show, not approve — M-1) →
  `resolve_proposal` with outbox/events/evidence handling. NOTE: as written it
  passes `ResolveOptions { auto_approve: true }`, which stamps entries
  `librarian_inferred` and silently skips summary-update conflicts
  (commit.rs:2051-2055) — see §2 for the review-path correction.
- A reject path exists only Tauri-command-shaped (`reject_wiki_page`,
  proposals_api.rs:169, DbState); no CLI/sidecar equivalent.
- Read surface: `ct proposals list|show` (show prints id/created_at/source
  paths/item payload JSON — NOT evidence quotes or proposed_name/kind; the
  data is in `StoredEvidenceChunk` on items, rendering does not exist yet).
- `ct status` already prints a `proposals_pending` count (ct.rs:420).
- The 35 pending proposals date from a Sep 4-era run; several carry
  malformed names ("Expo SDK 56" — wrong-corpus era).

**What is missing:** (1) the rules flip for the agents/ tree; (2) an operator
review loop — CLI interactive review and MCP surface for Tessera; (3)
reviewed-by provenance with real storage; (4) queue visibility in the nightly
run; (5) backlog triage for the 35; (6) an in-transaction pending guard the
current resolver lacks.

## Approach

### 1. Rules flip (data change, not code)

`folder_rules.auto_approve: 1 → 0` for the agents/ tree, at cutover (see
Rollout). Every subsequent librarian run parks its proposals as `pending`.
Rollback is the inverse update. Out of scope: other folders (none exist), a
config-file global flag, gating `vault_write_note` deposits (wisdom tier is
agent-authored BY DESIGN — this spec gates librarian synthesis only).

### 2. Review semantics (I-1): reviewed ≠ auto-approved

The review paths (CLI + MCP) call `resolve_proposal` with
`ResolveOptions { auto_approve: false, reviewed_by: Some(...) }` — mirroring
`proposals_api.rs approve_wiki_page`, which already does. Consequences, both
intended: entries are stamped `user_confirmed` (not `librarian_inferred`),
and summary-update conflicts SURFACE to the reviewer instead of being
silently skipped. The shared approve core moves from the tools crate
(cmds.rs) into `tauri_app_lib` (e.g. `db::proposals_review`) so the CLI, the
MCP dispatch, and the Tauri commands call ONE implementation; `cmds::
approve_one_on` becomes a thin wrapper (its auto semantics unchanged for
`ct approve`'s existing users).

"Mirroring `approve_wiki_page`" is the whole contract, not just the two
fields named above: the review core also plumbs the on-disk
`wiki.deposit_default_tier` (§3.2), as every other resolve path does.
Building `ResolveOptions` with `..Default::default()` silently omits it and
falls the resolver back to the shipped default, which would make an
operator's configured tier apply everywhere EXCEPT the review surfaces.

### 3. In-transaction pending guard (I-2)

`resolve_proposal` gains `UPDATE curated_proposals SET status = … WHERE id =
?1 AND status = 'pending'` (+ rows_affected == 1 check, else bail
"already resolved") as the final write, so a concurrent double-decide (CLI +
MCP) cannot re-resolve: the loser fails cleanly. Also hardens approve_all.
Sequential double-decide keeps the existing pre-check error.

### 4. Reviewed-by storage (I-3): columns, not event text

One migration adds `reviewed_by TEXT` to `curated_proposals`, symmetric with
the existing `reject_reason`/`resolved_at` columns. MCP decide's reject
`note` lands in the existing `reject_reason` column (its intended purpose).
The resolution event's summary text is display-only. Characterization to
record in tests: a fully-rejected new_entity proposal writes NO resolution
event (entity_id never materializes) — the columns, not the event log, are
the audit trail for decisions. `reviewed_by` is advisory metadata: absence
never blocks resolution; auto-approve leaves it NULL, keeping reviewed vs
auto queryable. The three `proposals_api.rs` ResolveOptions struct literals
(:74/:144/:202) gain `reviewed_by: None` lines (M-5).

Reviewer identity per surface: the desk stamps `desktop-ui`, the CLI stamps
the operator's OS account, and MCP stamps `<client>:<os-account>` (e.g.
`local-mcp:kurt`). The MCP connection label alone is fixed at startup and
identical for every caller, so it names the surface but cannot answer "who
approved this" — the question the column exists for. `auto_approve: false`
declares a human decision, so every path taking it names a reviewer,
including the legacy review-desk shims.

### 5. Review loop — CLI (operator)

New `ct proposals review`: interactive loop over pending —

```
[3/35] prop_378f…  proposed: @speechmatics/expo-two-way-audio
  (2 items, evidence: 3 chunks, doc: products/expo-llm-wiki.md)
  [y] approve  [n] reject  [d] detail  [s] skip  [q] quit
```

- `y` → review approve (§2 semantics). `n` → review reject (all items
  Rejected + reject_reason). `q`/`s` leave pending.
- `d` → full item bodies + evidence chunk quotes. Rendering is NEW work, not
  reuse (M-3): extend `proposals_show` (ct.rs:386) to print proposed_name,
  kind, and each item's `StoredEvidenceChunk` quotes/line-ranges. The
  extension serves both `review -d` and the §7 triage.
- Empty queue → "0 pending" exit 0 (cron-safe).

### 6. Review loop — MCP (Tessera)

Two new tools hosted in **src-tauri tool_dispatch** (the write-capable,
app-hosted server — NOT the standalone sidecar binary, whose primary
connection is read-only; I-4):

- `curated_proposals_list(status?: "pending"|"approved"|"rejected"|
  "partial"|"superseded", limit?)` — id, proposed_name, item count,
  evidence chunk count, source docs, created_at. Superseded included in the
  enum because `insert_proposal` actively supersedes stale pending
  proposals (proposals.rs:201/:221) — a queue can shrink for reasons other
  than decisions, and list/decide must handle it (M-2).
- `curated_proposal_decide(proposalId, decision: "approve"|"reject",
  note?: string)` — routes to the shared review core (§2), stamping
  reviewed_by + reject_reason per §4.

Authz: names start with `curated_` so the #187 deny (tool_dispatch.rs:1183)
covers them with zero new code; non-clanker clients (Hermes sidecar) pass —
which is the point: Tessera reviews. Both tools use the fail-closed
`log_agent_access_checked` audit path (tool_dispatch.rs:1147), not the
legacy best-effort logger. For `decide` the audit row is written INSIDE the
resolution transaction (`ReviewOptions::audit`), matching the wisdom write
tools: fail-closed means a failed audit aborts the decision, so a resolved
proposal with no log row is not a reachable state.

Two things `--mcp` cannot borrow from the desktop app, because neither
exists in a headless process:

- **Migrations.** The server's read connection is read-only and its lazy RW
  connection opens the file bare, so startup runs the migration ladder over
  the database explicitly (best-effort: a read-only database still serves
  reads, with the reason on stderr). Without it, the first write touching a
  migration-added column — `reviewed_by` itself — fails until the desktop
  app or a CLI command happens to open the file.
- **The embedding sweep.** `run_embedding_sweep` takes a `DbState` and runs
  only in the Tauri app, so "the sweep will fill it later" is not available
  here. `decide` precomputes entry embeddings BEFORE taking the write lock
  (the desk's three-phase shape) rather than committing NULL-embedded facts
  that nothing would ever re-embed.

Escalation pattern (no code): Tessera's nightly run lists pending; proposals
it cannot confidently adjudicate (novel claims, cross-entity merges) are
posted to Kurt in Discord with evidence attached; Kurt's reply
(approve/reject + id) is executed via the decide tool.

### 7. Nightly queue visibility + backlog triage

Nightly cron prompt (Tessera-side, deliberately not repo code): call
`curated_proposals_list(pending)` — the single source of truth for the
morning number (M-6; `ct status`'s count is noted but not double-reported) —
review/escalate per §6, flag backlog > 20.

Backlog (cutover, one-time): the 35 pending proposals are triaged with the
extended `proposals_show` + MCP decide — approve the well-formed minority,
reject the malformed with notes, report the tally to Kurt. No extra code.

## Rollout order (M-4)

1. Land this PR (review core + guard + columns + CLI + MCP + show
   extension).
2. Release + install.
3. Edit the nightly cron prompt (tool now exists; editing before install
   would make the nightly call fail).
4. Cutover: flip `folder_rules.auto_approve → 0` (Tessera, one SQL
   statement, logged in session record), triage the 35 (§7).
5. First morning summary with the pending-queue line reaches Kurt.

Ordering hazard, stated so the runbook can't get it wrong: shipping the loop
with the rule still 1 is HARMLESS (status-quo auto-approve); flipping the
rule BEFORE install is the dangerous order (pending accumulates with no
review surface). The order above avoids it.

Rollback: flip the rule back to 1; the review loop keeps working for anyone
who calls it.

## Error handling

- decide on non-pending (incl. superseded) proposal → explicit error from
  the §3 guard (sequential: pre-check message; concurrent: in-tx guard
  message).
- reviewed_by is advisory; absence never blocks resolution.
- proposals list on a fresh brain → empty array, not error.

## Testing

1. Folder-rule gate (integration, librarian/mod.rs pattern): auto_approve=0
   → proposal pending, ZERO new llm_wiki_entries; =1 → committed
   (characterization).
2. CLI review round-trip: y → entries stamped `user_confirmed` (T-3),
   events + outbox consistent, conflicts NOT silently skipped; n →
   rejected + reject_reason, no entries; empty-queue exit 0.
3. In-tx guard (T-1): resolve against an already-resolved id FAILS (both
   sequential and simulated-concurrent double-decide), no second resolution
   event, no edge re-inserts.
4. new_entity full-reject characterization (T-2): no resolution event;
   reviewed_by/reject_reason persist in columns.
5. supersede interaction (T-4): superseded proposal hidden from default
   list, decide on it errors cleanly.
6. MCP tools: list shapes/filters; decide approve stamps reviewed_by +
   user_confirmed; decide reject writes reject_reason; authz regression
   (clanker-bridge denied, per the #187 test pattern); fail-closed audit
   rows written.
7. Migration: reviewed_by column added; existing rows NULL; CHECK state
   machine untouched.

## Out of scope

- Tauri app review UI (existing proposal viewer renders pending; review
  buttons can follow).
- Per-item accept/reject in the CLI (whole-proposal granularity matches the
  batch shape; item-level lives in the app).
- Any change to wisdom-tier deposits or vault_write_note.
- Auto-expiry of stale pending proposals (nightly flag covers the symptom).
- Hosting write tools on the standalone sidecar binary (its read-only
  connection posture is a feature; revisit only with a real need).
