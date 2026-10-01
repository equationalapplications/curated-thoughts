# INTENT — Curated Thoughts (The Brain, The Vault, and Trust Boundaries)

**Read this file first.** It explains why CT exists, the business rules its
components must enforce, what is explicitly out of scope, and how changes are
made. When this file and any other document disagree, this file wins on
*intent*; specs win on *detail*.

## Why CT exists

Curated Thoughts is a privacy-first, local-first second brain for humans of
authority (engineers, product managers, authors — anyone whose words are canon)
and for the agents that serve them. Raw material is deposited as real files;
the **Active Librarian** distills it into a curated wisdom layer that any agent
— including a weak one — can recall intuitively. Structure does the work, not
model cleverness. Two surfaces over one pipeline: the **app** (user-friendly,
any human author, any genre) and the **sidecar/MCP/CLI** (opinionated for
software development and agents). "Agent-facing" in this file means BOTH the
MCP sidecar AND the `ct` CLI.

## Business rules (non-negotiable)

1. **The Trust Boundary (file-first deposit).** Agents must NEVER write
   directly to the curated wisdom layer (the database). All agentic knowledge
   is written to the vault as files via `wisdom_deposit`; the Active Librarian
   is the sole authority that reads those files, understands them, and stamps
   them into the brain as facts. Removed from the agent surface (MCP and CLI
   alike): `curated_add_wisdom`, `curated_update_wisdom`,
   `curated_proposal_decide`, `curated_archive_wisdom` — no agent-reachable
   path may insert, update, approve, or archive wisdom rows.
2. **Attestation over location (tier = f(provenance class)).** Human tier
   requires a `human_attestations` record (written only by app UI flows, keyed
   by content_hash). Every unattested file ingests at agent tier, wherever it
   sits. No mechanism detects "an agent wrote this"; the guarantee is honest:
   impossible through sanctioned surfaces, detectable through unsanctioned
   ones (provenance mismatch). Migration: existing human ISF is bulk-attested
   once; laundered `user_stated` rows are identified, re-ingested as agent
   files, and superseded.
3. **The visibility race.** Every deposit reports `pending` (queryable via
   `wisdom_deposit_status`) until ingest completes; a host with no ingest
   capability reports `pending: no ingest host`. Deposit kicks ingest — the
   deposit path requests immediate ingestion of the file it just landed (the
   ingest channel itself is new work). We do NOT scan uningested files in
   injection or ranked recall; the only exception is the on-demand, read-only
   `wisdom_pending` listing, which is never merged into recall results or the
   injection block.
4. **Retirement is supersession, not deletion; authority follows provenance.**
   Agents retire wisdom ONLY by depositing a supersession file
   (`wisdom_propose_supersession`). The Active Librarian applies it
   automatically when both facts are agent tier; when the target is human
   tier, it becomes a proposal that only a human resolves in the app.
5. **The fleet shares one wisdom layer via the outbox.** Reconcile is
   origin-scoped (rows carry `origin_host`): only the host that ingested a
   fact may judge its source gone. Fact ids are deterministic, hashed from the
   vault-relative, normalized doc path + content hash + extraction index, so
   duplicate ingest across hosts converges. Peers never upgrade a provenance
   stamp; how peers trust incoming stamps is an open decision (RR-2) — until
   then, attestation records sync alongside the facts they cover.
6. **System One judges; it never authors.** Fast, cheap, typed judgment
   (Jev/Laya) is used for ingest triage and recall relevance, audit-logged for
   evals. It never writes facts, approves anything, rewrites queries, or
   decides trust. It is optional; recall works without it.
7. **Recall abstains; it does not bluff.** A weak top hit is treated as "no
   answer," never confidently injected. Question embeddings are generated at
   ingest; the abstention floor is a fixed, benchmark-calibrated threshold.
8. **Scope follows the work.** Facts ingested under `works/<project>/` stay
   scoped to that work and never surface in other scopes' recall. Human-stated
   captures live under `stated/<author>/YYYY/`, are app-authored (never
   agent-authored), carry `stated_by`/`captured_via`/`attestation_id`
   frontmatter, and stay below wisdom tier until attested.
9. **Vault semantics by area.** `agents/` is append-only for agents: deposits
   are never edited or deleted; corrections are new supersession deposits.
   Human areas are edited freely by humans, with git as the lineage. Reconcile
   retires facts whose source is gone — and only on the ingesting host.

## Active Librarian functions (decided, not yet built)

The component is the **Active Librarian** with two functions: **ingest**
(understand and stamp) and **reconcile** (repair, consistency flagging,
supersession upkeep). "Heal" is retired as a term. Committed enhancements:
deterministic fact ids (guarantee dedup at retrieval time); ingest-time
"questions this fact answers" embeddings, recall taking the max over a fact's
vectors; supersession/current-only filtering in Rust recall (migration-13
columns; Rust readability is RR-4, open). Corpus design target: ~10⁴ live
facts per brain; ANN deferred to 10⁵.

## Non-goals (do not build here)

- No generalizing the MCP/CLI surface for non-developer genres — the app owns
  that experience.
- No routing recall through the TypeScript engine; port engine capabilities
  into Rust recall instead.
- No graph-traversal expansion inside session-start injection (v1); traversal
  stays in on-demand tools (`wiki_context`).
- No reading of uningested files in injection or ranked recall (see rule 3 for
  the `wisdom_pending` exception).
- No provenance used as a hidden ranking penalty; provenance is stamped and
  surfaced (CTI labels it), and any ranking change must be decided by a spec
  against the benchmarks.
- No deletion of source files as part of fact retirement.

## Workflow (how changes are made here)

1. Spec first under `docs/superpowers/specs/`, from a `[V]`-evidenced
   investigation (claims read from real source, not memory).
2. Canonical decision context lives in the equational-wiki vault
   (`wisdom-deposit-file-first-intent-2026-10-01.md`, the Opus decision brief
   and cycle-2 review, 2026-10-01).
3. Dual review (GLM + Opus) → CI green → merge; open questions park the PR.
4. Recall/injection changes gate on the engine benchmarks: supersession
   scenarios (no superseded fact is ever injected), LongMemEval hit@k +
   abstention precision, cross-model paraphrase probes (n ≥ 200),
   exactly-once property tests, cache-safety byte-stability tests.
   Reconcile/ingest changes gate on the two-host fleet test (origin-scoped
   reconcile; duplicate ingest produces no duplicate rows). Benchmarks run on
   scratch brains with real embeddings; embedding stubs are for unit tests
   only.
5. Never touch the live brain out-of-band; tests use scratch profiles.
