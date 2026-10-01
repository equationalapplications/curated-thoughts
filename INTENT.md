# INTENT — Curated Thoughts

**Read this file first.** It explains why CT exists, the business rules its
components must enforce, what is explicitly out of scope, and how changes are
made here. When this file and any other document disagree, this file wins on
*intent*; specs win on *detail*.

## Why CT exists

Curated Thoughts is a privacy-first, local-first second brain for humans of
authority (engineers, product managers, authors — anyone whose words are
canon) and for the agents that serve them. It is built on the LLM Wiki idea:
raw material is deposited as real files; the **Active Librarian** distills it
into a curated, traversable wisdom layer that any agent — including a weak one —
can recall intuitively. Structure does the work, not model cleverness.

The product has two surfaces over one pipeline:

- **The app**: user-friendly, for any human author (including non-developers,
  e.g. a fiction author curating a world canon).
- **Sidecar / MCP / CLI**: opinionated for software development and agents.

## Business rules (non-negotiable)

1. **Trust comes from provenance, never from path or prose.** Content reaches
   human-trust tier only through an authenticated human action in the app
   (attestation). Everything else — whatever folder it sits in — is
   agent-provenance: labeled, lower-weighted, and reviewable. No file path, tool
   name, or `user_stated` label can manufacture trust. (This closes the
   "trust inversion" found 2026-10-01: `curated_add_wisdom` minted
   `confidence='confirmed'`, `source_type='user_stated'` rows from agent calls.)
2. **Agents deposit as files, never as database rows.** The agent wisdom write
   path lands a file under `immutable-source-files/agents/`; the Active
   Librarian ingests it like any other source and stamps its provenance class.
   No agent-facing tool may insert, update, approve, or archive wisdom rows
   directly. Approval (`curated_proposal_decide`-class authority) is human-only.
3. **Auto-ingest is the default.** Deposits become recallable without a human
   gate; correctness is enforced *after* landing by reconcile (consistency
   flagging into the proposals queue) and by tier weighting — not by blocking
   the door. Human-tier targeting (corrections, supersessions of attested
   facts, "someone said" quotes) goes through proposals and waits for a human.
4. **Retirement is supersession, not deletion.** Wrong wisdom is superseded by
   corrected wisdom; history stays auditable. Agents may propose supersessions
   of anything and apply them only agent→agent; human-tier facts are retired
   only by a human in the app.
5. **The fleet shares one wisdom layer via the outbox.** Rows sync between
   peers; heal/reconcile is origin-scoped — only the host that ingested a fact
   may judge its source dead. Deterministic fact ids (from doc path + content
   hash + extraction index) prevent duplicate ingest across hosts.
6. **System One (Jev/Laya) judges; it never authors.** Fast, cheap, typed
   judgment is used for ingest triage and recall relevance, audit-logged for
   evals. It never writes facts, approves anything, rewrites queries on the hot
   path, or decides trust.
7. **Recall abstains; it does not bluff.** A weak top hit is treated as "no
   answer" rather than confidently injected. Retrieval quality work (multi-vector
   question embeddings, score floors) happens at ingest time, keeping the query
   path deterministic.

## Non-goals (do not build these here)

- No generalizing the MCP/CLI surface for non-developer genres; the app owns
  that experience.
- No routing recall through the TypeScript engine (no Node runtime on fleet
  hosts); engine capabilities are ported into Rust recall where needed.
- No graph-traversal expansion inside session-start injection (v1); traversal
  stays in on-demand recall tools.
- No scan of uningested files inside the injection path; freshness is handled
  by deposit-kicked ingest, never by query-time file reading.
- No silent trust upgrades on sync: receiving peers verify or downgrade
  provenance stamps they cannot attest to.
- No deletion of source files as part of fact retirement; the vault is
  append-only, git is the lineage.

## Workflow (how changes are made here)

1. **Spec first.** Every behavioral change starts as a spec under
   `docs/superpowers/specs/`, grown from an investigation with evidence
   (`[V]`-tagged claims read from real source, not memory).
2. **Read the context.** Canonical architecture context lives in the
   equational-wiki vault: `immutable-source-files/agents/memories/wisdom-deposit-file-first-intent-2026-10-01.md`
   (decision log), `records/operations/2026-10-01-wisdom-architecture-opus-decision-brief.md`
   and `...-opus-review-c2.md` (the options analysis this INTENT implements).
3. **Dual review.** Changes go through the delivery flow: investigation → spec →
   plan → TDD implementation → dual review (GLM + Opus) → CI green → merge.
   Open questions park the PR; nothing merges past one.
4. **Benchmarks gate recall changes.** Recall/injection changes are tested with
   the engine benchmarks package: supersession scenarios (no superseded fact is
   ever injected), paraphrase probes authored by a *different* model than the
   seed author (n ≥ 200), exactly-once property tests, and byte-stability
   golden tests for cache safety.
5. **Never touch the live brain out-of-band.** The sidecar/DB is written by the
   pipeline only; tests use scratch profiles and stub embeddings.
