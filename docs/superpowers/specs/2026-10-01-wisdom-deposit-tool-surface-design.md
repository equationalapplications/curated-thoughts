# Design — wisdom_deposit tool surface + agent write-path removal (2026-10-01, rev 2)

Constitution: `INTENT.md` v2 on main (`638b27a`; rules cited below are v2
numbering — the rev-1 v1/v2 drift is closed by landing #256 first).
Investigation evidence base:
`2026-10-01-wisdom-deposit-tool-surface-investigation.md` (§2.2 corrected in
its rev-2 addendum). Scope: surface + deposit path only. B1 attestation,
reconcile, recall/ranking, app UI are out of scope.

**Rev 2 applies the full independent review (REQUEST CHANGES, 9 MAJOR /
6 MINOR, `~/.local/state/opus-review/2026-10-01-wisdom-deposit-spec-c1.json`).
The structural change: deposits are ORDINARY SOURCE DOCUMENTS for the Active
Librarian — the rev-1 idea that ingest parses the fact-file format was wrong
(`parse_fact_file` is only reachable via OKF bundle import) and the
doc-path-keyed status query was built on a key the pipeline never stamps
(facts carry `librarian-<hash32>` tokens; the doc link lives in
`librarian_evidence` + chunks).**

## 1. Decisions

### D1 — Tool set (canonical names per INTENT)

MCP sidecar surface: remove `curated_add_wisdom`, `curated_update_wisdom`,
`curated_archive_wisdom`, `curated_proposal_decide`; add:

| Tool | Kind | Purpose |
|---|---|---|
| `wisdom_deposit` | write (file) | Append-only deposit of a source document under `immutable-source-files/agents/`; kicks ingest; returns `pending` |
| `wisdom_deposit_status` | read | Ingest state of one deposited file (by its vault-relative path) |
| `wisdom_propose_supersession` | write (file) | Deposit a supersession file targeting an existing fact; the Active Librarian applies it (rule 4) |
| `wisdom_pending` | read-only listing | Uningested deposits; **never** merged into recall or injection (rule 3) |

Net tool count stays 16; `mcp_integration.rs` keeps its exact-name assertion
with the updated list.

**Guard-prefix updates (review F6):** the cloud-bridge remote-session deny
(`tool_dispatch.rs:1432`) and the fail-closed agent-access audit gate
(`tool_dispatch.rs:1618`) both match `curated_` prefixes only. Both become
`curated_` OR `wisdom_` — `wisdom_deposit` must be remotely-uncallable and
audited exactly like the tools it replaces. The existing bridge tests
(`lib.rs` bridge-denial block) gain `wisdom_*` cases.

### D2 — `wisdom_deposit` contract

Parameters: `path` (vault-relative, MUST normalize under
`immutable-source-files/agents/`, and MUST NOT land under
`agents/supersessions/` — that lane is `wisdom_propose_supersession`-only,
review F11), `title`, `body` (markdown), `tags` (optional).

The deposit is a **plain, well-formed source document** (title heading +
markdown body; tags rendered as a `tags:` frontmatter line using the existing
OKF serializer). The Active Librarian reads it like any vault document — no
new ingest format code, no pre-stamped fact id. Provenance is a property of
the folder + attestation state (rule 2), not of frontmatter the agent
composes; the agent cannot claim tiers or ids through file contents.

- **Append-only enforcement (rule 9), two legs:**
  1. `wisdom_deposit` refuses an existing target path (`deposit_exists`);
     corrections go through `wisdom_propose_supersession`.
  2. `vault_write_note` (which CAN currently edit `agents/**` —
     `NOTE_WRITABLE_SUBDIRS` includes `AGENTS_DEPOSIT_DIR`) refuses any
     If-Match edit under `immutable-source-files/agents/` (`agents_immutable`
     error, review F7). Human edits to agent deposits go through git, not the
     tool. New writes (non-existing paths) under `agents/` via
     `vault_write_note` are also refused — `wisdom_deposit` is the only
     sanctioned creator, so hand-shaped fact files can't bypass the
     tool-owned shape.
- Symlink escape refusal: verified existing behavior
  (`safe_vault_path` + `create_parents_no_symlink`, `okf/write.rs:355`, tests
  at `:1402-1430`) — deposit reuses that stack; test pins it (review F14).

Return: `{ path, pending: true, kick: "started" | "queued_watcher" |
"no_ingest_host" }` (semantics in D3).

### D3 — Deposit→ingest kick (review F1/F3/F8 redesign)

The kick must run the **full per-document pipeline** — chunk/embed AND the
Active Librarian — because chunk rows alone never produce wisdom (the
Librarian runs inside the pipeline worker, `pipeline/mod.rs:243`; the rev-1
bare-`ingest_document_with_vault_root` kick would strand every deposit in
`pending` forever).

- **Folder rule (review F3):** a migration seeds
  `folder_rules` for `immutable-source-files/agents/` with
  `librarian_mode = <fact-producing mode, pinned from librarian/mod.rs at
  implementation>` and `auto_approve = true`. Without it, every deposit waits
  on a human (`get_folder_mode` defaults to `("summarize", false)`) — the
  opposite of rule 3. Auto-approve on this folder is trustworthy precisely
  because the folder is agent-class: the seeding migration's test asserts the
  stamped rows carry **agent** provenance (no `user_stated`-class label) and
  surface in recall — auto-approve must not recreate the rule-1 trust
  inversion.
- **Execution model:** on deposit, the sidecar spawns a background task that
  (1) takes the brain `VaultLock` (blocking a concurrent app/watcher run for
  the duration — the same mutual exclusion every ingest path uses; review
  F8b), (2) runs the per-document ingest + librarian sequence for exactly the
  deposited file, (3) releases the lock. If the lock is already held
  (watcher/app mid-run), the task does NOT run: the deposit returns
  `kick: "queued_watcher"` — the watcher's file event covers the file.
- **Kick values (review F8a):**
  - `started` — task spawned and will run (lock acquired).
  - `queued_watcher` — lock held by a running watcher; its event loop owns
    the file.
  - `no_ingest_host` — embedding/gen endpoints unconfigured (cheap config
    probe first). File still written; deposit must not fail because ingest
    can't run (rule 3's honest pending).
  No conflation: `queued_watcher` means "will be ingested by the running
  host", `no_ingest_host` means "nothing will ingest until configured".
- **Recovery (review F8a):** `ct watch` and the sidecar startup path gain a
  **catch-up sweep** — scan `immutable-source-files/agents/` for deposits
  with no librarian rows (the D6 query) and ingest them. A file deposited
  while nothing was running gets picked up at the next host start; no file
  can strand silently.
- **Failure visibility (review F8c):** spawned-task errors (embedding
  failure, panic, missing RW connection) are logged AND recorded;
  `wisdom_deposit_status` reports `failed` with the error text. A deposit
  never sits in silent `pending` after a kick failure.
- **RW access (review F8d):** the kick uses the lazy RW connection
  (`with_rw`), not the sidecar's read-only `ctx.conn`; a missing-DB open
  failure surfaces as `failed`, not a hang.
- **Worker post-steps:** the kick's librarian run covers the same steps the
  worker runs for a document (including `pending_linkers` upkeep). Any step
  deliberately skipped must be listed in the implementation PR with the
  reason; "no new ingest semantics" means *reusing the worker's per-document
  functions*, not calling a truncated subset.

### D4 — `wisdom_deposit_status` (review F1 redesign)

Truth source = the brain, keyed the way the pipeline actually keys:

- librarian rows exist whose `librarian_evidence` links to the deposit path
  (the doc-path link lives in `librarian_evidence.evidence_json` + chunks
  since V18 — `source_ref` only carries `librarian-<hash32>` tokens) →
  `ingested` (+ fact ids, tier, supersession state; RR-4 caveat below)
- chunk rows exist for `doc_path`, no librarian rows → `chunked`
- kick recorded a failure → `failed` (+ error)
- file exists, nothing else → `pending`
- no such file → `unknown`

The exact evidence-link query is pinned at implementation with a test (seed a
scratch brain via a real ingest+librarian run, assert the query finds it);
review F15's corrected investigation claim is the citation of record.

### D5 — `wisdom_propose_supersession`

Parameters: `target_source_ref` (a fact's `librarian-…` source_ref token)
**or** `target_fact_id` — exactly one must be supplied (error otherwise) —
plus `replacement_title`, `replacement_body`, `reason`.

Writes a supersession deposit file under
`immutable-source-files/agents/supersessions/` carrying `supersedes:
<target>` frontmatter (written by the tool; not agent-composed). The tool
does NOT touch brain rows — application is the Active Librarian's job
(rule 4): automatic when the target is agent tier, a human-resolved proposal
when human tier. Librarian application is a follow-up spec; until it lands,
`wisdom_deposit_status` on a supersession file reports `pending`, and the
supersessions lane is excluded from the D3 folder rule (it ingests as an
ordinary document, producing no facts via the fact-mode rule — its lane
semantics arrive with the application spec).

### D6 — `wisdom_pending` (review F1-aligned)

Read-only: list deposit files under `immutable-source-files/agents/` with no
librarian rows linked via `librarian_evidence` (the D4 query, batched), each
with `{ path, deposited_at (file mtime), kick }`. Isolation from recall is
structural — separate tool, separate code path, plus the existing grep-level
guard test pattern.

### D7 — CLI parity + closing every agent-reachable approve path

`tools/src/bin/ct.rs` gains a `wisdom` subcommand group mirroring the MCP
surface: `ct wisdom deposit|status|propose-supersession|pending` (same
dispatchers, `--json` output, `--yes` on the write subcommands).

Removed agent-reachable write/approve paths (review F4/F5/F13):

- `ct approve` — removed (rev 1).
- **`ct proposals review` gains a TTY gate** (review F4): refuses unless
  stdin is a terminal (`is_terminal()`; fail-closed). The Human Verification
  Gate is human-at-keyboard by definition; `yes y | ct proposals review`
  must error, with a test.
- **`tools/src/bin/approve_pending_proposals.rs` is deleted** (review F5):
  Cargo auto-discovers it (16 bin files vs 13 `[[bin]]` entries, autobins
  on) and it calls `approve_all()` non-interactively. `approve_all`/
  `approve_one` then lose their last callers and are deleted.
- **Named exceptions, deferred by name (review F13):** `ct wiki forget
  --yes` (incident cleanup hard-delete) and `ct heal --yes` stay in this PR
  but are listed here as deliberate, temporary exceptions to rule 4's
  agent-no-delete surface. They close with the reconcile PR, which replaces
  `heal` with origin-scoped reconcile outright and moves `wiki forget`
  behind the same human gate as proposal resolution. No new delete surface
  may be added while these exceptions stand.

### D8 — Docs sweep (review F10)

All agent-facing text advertising the removed tools flips in the same PR:
`.skills/curated-thoughts/skill.md` (lines advertising
`curated_add_wisdom` — the file agents actually load),
`tools/src/bin/curated_thoughts_mcp.rs:584` setup instructions, README,
CHANGELOG note, and any spec text this change orphans. RR-3 remains the full
parity audit.

## 2. Tests (CI-gated)

1. `mcp_integration.rs`: exact 16-name list (updated); sweep ALL FOUR removed
   names out of the integration tests; deposit round-trip on a scratch brain
   with real embeddings per repo benchmark policy: deposit → file on disk →
   kick runs ingest+librarian → status `ingested` with fact ids + agent
   provenance asserted.
2. Append-only: deposit to an existing path errors; deposit outside
   `immutable-source-files/agents/` errors; deposit INTO
   `agents/supersessions/` via `wisdom_deposit` errors; symlink-escape
   deposit errors; **`vault_write_note` refuses new writes AND If-Match
   edits under `immutable-source-files/agents/`**.
3. Kick semantics: unconfigured endpoints → `no_ingest_host` (file still
   written); lock held → `queued_watcher`; spawned-task failure → status
   `failed` with error text; watcher+kick mutual exclusion via VaultLock
   (concurrent run test).
4. Catch-up sweep: deposit file with no librarian rows is picked up on
   `ct watch` startup.
5. Supersession: file lands in `supersessions/` with `supersedes`
   frontmatter; exactly-one target enforced; no brain mutation (row count
   unchanged).
6. Approval closure: `yes y | ct proposals review` exits with a refusal
   (TTY gate); `approve_pending_proposals` binary gone (build-level: bin
   count assert); `ct approve` unknown-command.
7. Bridge + audit: cloud-bridge session calling `wisdom_deposit` denied;
   deposit/supersession writes present in the fail-closed agent-access audit.
8. Folder rule migration: `immutable-source-files/agents/` rule seeded with
   auto_approve; librarian run over a deposit stamps agent-tier rows that
   recall returns (no human-tier label anywhere).
9. Benchmark gates: supersession scenarios etc. fire only when the recall
   consumption PR lands; this PR changes no ranked-recall semantics (the
   librarian runs only over the deposited document, as the worker already
   does for any document).

## 3. Out of scope / deferred (each to its named follow-up)

- B1 attestation design + `human_attestations` + bulk-attestation migration +
  ~21 laundered-row re-ingestion (next spec per handoff).
- Librarian application of supersession deposits (rule 4 mechanics, fleet
  origin-scoping) — reconcile PR (which also closes the D7 wiki-forget/heal
  exceptions).
- Recall/injection consumption of supersession state + abstention floor —
  benchmark-gated recall PRs.
- `stated/<author>/YYYY/` human lane — app-owned (rule 8), not agent surface.

## 4. Risks

- RR-4 (Rust readability of migration-13 columns): D4 reports tier/
  supersession enrichment only if the column read is verified at
  implementation; otherwise `ingested` without enrichment. No silent wrong
  answers either way.
- Removing the write tools breaks every agent pattern that calls them — that
  is the point (rule 1); the D8 sweep flips our own docs in the same PR.
- The D3 kick runs librarian steps outside the pipeline worker; the
  VaultLock discipline (one runner at a time) is what keeps this from racing
  the watcher — test 3 pins it. If implementation proves the kick needs
  worker plumbing that doesn't decompose per-document, the fallback is:
  deposit returns `queued_worker`, and the kick becomes a nudge to the
  running worker (or `ct watch` start) — still honest per rule 3, but that
  fallback must be surfaced in the PR, not swapped in silently.
