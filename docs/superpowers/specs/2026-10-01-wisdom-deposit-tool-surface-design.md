# Design — wisdom_deposit tool surface + agent write-path removal (2026-10-01)

Constitution: `INTENT.md` v2 (rule 1 trust boundary, rule 3 visibility race,
rule 4 supersession, rule 8 scoping, rule 9 append-only). Investigation (evidence
base, `[V]` citations): `2026-10-01-wisdom-deposit-tool-surface-investigation.md`
(same directory). Scope follows the work: **surface + deposit path only**. B1
attestation, reconcile, recall/ranking, and app UI are explicitly out of scope.

## 1. Decisions

### D1 — Tool set (canonical names per INTENT)

MCP sidecar surface: remove `curated_add_wisdom`, `curated_update_wisdom`,
`curated_archive_wisdom`, `curated_proposal_decide`; add:

| Tool | Kind | Purpose |
|---|---|---|
| `wisdom_deposit` | write (file) | Append-only deposit of a fact file under `immutable-source-files/agents/`; kicks ingest; returns `pending` |
| `wisdom_deposit_status` | read | Ingest state of one deposited file (by its vault-relative path) |
| `wisdom_propose_supersession` | write (file) | Deposit a supersession file targeting an existing fact; the Active Librarian applies it (rule 4) |
| `wisdom_pending` | read-only listing | Uningested deposits; **never** merged into recall or injection (rule 3) |

Net tool count stays 16 (−4/+4); `mcp_integration.rs` keeps its exact-name
assertion with the updated list.

### D2 — `wisdom_deposit` contract

Parameters: `path` (vault-relative, MUST normalize under
`immutable-source-files/agents/`), `title`, `body` (markdown), `tags` (optional
string list). The tool — not the calling agent — owns the file shape:

- Frontmatter is built with the existing fact-file writer
  (`okf/fact_file.rs::build_fact_file`, profile `llm-wiki/2` provenance keys),
  so ingest parses deposits with **zero new format code**.
- `author`/provenance keys stamp the **agent** class. No `human_attestations`
  row is created or implied (B1 is a separate spec; every deposit ingests at
  agent tier per rule 2).
- `id` is computed deterministically at deposit time:
  `hash(vault-relative normalized path + content hash)` (extraction index comes
  in at ingest; rule 5's convergence property is preserved — the same deposit
  from two hosts computes the same id).
- **Append-only enforcement (rule 9):** an existing target path is a hard
  error (`deposit_exists`). Corrections go through
  `wisdom_propose_supersession`. This is the opposite of `vault_write_note`'s
  If-Match editing — deposits have no edit path at all.
- Path guard reuses the writable-subdir resolution already enforced in
  `okf/write.rs`; symlink escapes are refused by the existing
  `trusted_links` verdict path.

Return: `{ path, id, pending: true, kick: <"started" | "no_ingest_host"> }`.

### D3 — Deposit→ingest kick (C6; decision on investigation Q1)

Chosen: **in-process single-document ingest, spawned async**, with honest
degradation.

- On deposit, the sidecar spawns a background task calling the existing
  `pipeline::ingest_document_with_vault_root` (investigation §3) for exactly
  the deposited file — the same code path `ct ingest` uses, so no new ingest
  semantics and the fleet/ingest benchmark gates stay out of scope.
- If the brain write lock is contended (a watcher/ingest host is mid-run), the
  kick reports `kick: "no_ingest_host"` — the file sits uningested exactly as
  rule 3 prescribes; a running `ct watch` daemon ingests it via its natural
  file event. We do NOT build a new cross-process channel in this spec; that
  remains open for the reconcile/ingest PR if the fleet needs it.
- Embedding/gen endpoints unconfigured → same `no_ingest_host` answer
  (config probe, cheap, before spawning).

### D4 — `wisdom_deposit_status` (investigation Q3)

Truth source = the brain, queried by `source_ref` (deposited fact files carry
their vault-relative path in `resource`/source_ref; ingest stamps entries with
it — investigation §2.2):

- rows exist for `source_ref = path` → `ingested` (+ entry id, tier,
  supersession state from the migration-13 columns **read via the existing
  mirror**, flagged RR-4 if the Rust read hits the verified gap — see §5)
- file exists, no rows → `pending` (+ `kick` result from D3's record)
- no such file → `unknown`

In-flight state needs no new table: pending-until-rows is the honest state; the
kick's spawn outcome is returned at deposit time and remembered in-process for
the life of the sidecar (best-effort; after a sidecar restart, plain `pending`
is still truthful).

### D5 — `wisdom_propose_supersession` (investigation Q2/Q5 adjacency)

Parameters: `target_source_ref` (the deposited fact's vault-relative path) or
`target_fact_id`, `replacement_title`, `replacement_body`, `reason`.

Writes a supersession deposit file under
`immutable-source-files/agents/supersessions/` with frontmatter
`supersedes: <target source_ref or fact id>` and the same agent provenance
stamping as D2. The tool does NOT touch brain rows — application is the Active
Librarian's job (rule 4): automatic when the target is agent tier, a
human-resolved proposal when human tier. The Librarian-side application logic
is a follow-up spec; this PR ships the deposit surface only, and
`wisdom_deposit_status` on a supersession file reports `pending` until that
spec lands.

### D6 — `wisdom_pending` (investigation Q5)

Read-only: list deposit files under `immutable-source-files/agents/` that have
no brain rows for their `source_ref` (the D4 query, batched), each with
`{ path, deposited_at (file mtime), kick }`. Isolation from recall is
structural — separate tool, separate code path, and a test asserts no
recall/injection code path references it (grep-level guard test, mirroring the
existing tools/list exact-name discipline).

### D7 — CLI parity (investigation Q4; INTENT: "MCP and CLI alike")

`tools/src/bin/ct.rs` gains a `wisdom` subcommand group mirroring the MCP
surface: `ct wisdom deposit|status|propose-supersession|pending` (same
dispatchers, `--json` output, `--yes` on `deposit`/`propose-supersession`
matching the CLI's existing write-confirmation convention).

`ct approve` is **removed** (it is the non-interactive shell-accessible
embodiment of `curated_proposal_decide`; an agent can run any CLI). The
interactive Human Verification Gate `ct proposals review` survives untouched —
it is a human-at-keyboard flow and becomes the CLI leg of the rule-4 proposal
resolution until the app UI owns it (B1). `cli_common::approve_one/approve_all`
lose their last caller and are deleted.

### D8 — Second MCP binary + docs sweep (investigation §1.2, §6)

- `tools/src/bin/curated_thoughts_mcp.rs:584`: setup-instructions line
  advertising `curated_add_wisdom` → advertise `wisdom_deposit` (agent
  onboarding must not teach a removed tool).
- README/tool-list docs sweep for the four removed names (RR-3 covers a full
  parity audit; this PR fixes the references this change orphans).

## 2. Tests (CI-gated)

1. `mcp_integration.rs`: exact 16-name list (updated); remove the
   add/update round-trip tests; add deposit round-trip: deposit → file exists
   on disk with agent provenance keys → `parse_fact_file` round-trips → status
   `pending` → (with stub embeddings) ingest runs → status `ingested`.
2. Append-only: deposit to an existing path errors; deposit outside
   `immutable-source-files/agents/` errors; symlink-escape deposit errors.
3. Kick degradation: endpoints unconfigured → `no_ingest_host`, file still
   written (deposit must not fail because ingest can't run — rule 3's honest
   pending).
4. `wisdom_pending`: lists the uningested deposit; empty after ingest; not
   referenced by any recall/injection path (guard test).
5. Supersession: file lands under `supersessions/` with `supersedes`
   frontmatter; no brain mutation (assert row count unchanged).
6. CLI parity: `ct wisdom --help` shows the four subcommands; `ct approve`
   exits with an unknown-command error.
7. Benchmark gates: none fire — no recall, injection, or reconcile semantics
   change in this PR (kick reuses `ingest_document_with_vault_root` verbatim).

## 3. Out of scope / deferred (each to its named follow-up)

- B1 attestation design + `human_attestations` + bulk-attestation migration +
  the ~21 laundered-row re-ingestion (separate spec, next per handoff).
- Librarian application of supersession deposits (rule 4 mechanics, fleet
  origin-scoping) — reconcile PR.
- Recall/injection consumption of supersession state + abstention floor —
  benchmark-gated recall PRs.
- Cross-process kick channel (watcher/sidecar) — only if the fleet test
  demands it after reconcile lands.
- `stated/<author>/YYYY/` human lane — app-owned (rule 8), not agent surface.

## 4. Risks

- RR-4 (Rust readability of migration-13 columns): D4 reads only
  `source_ref`-keyed existence + ids in this PR; if the tier/supersession read
  proves unverified, status reports `ingested` without tier enrichment and the
  enrichment lands with the recall PR. No silent wrong answers either way.
- Removing the write tools breaks every agent prompt/pattern in the wild that
  calls them — that is the point (rule 1), but the setup-instructions text
  (D8) and this repo's own plugin docs must flip in the same PR so our own
  agents never see a stale advertisement.
