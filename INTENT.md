# INTENT — Curated Thoughts

**Read this file first.** It explains why Curated Thoughts (CT) exists, the
rules it must follow, what it deliberately does not do, and how changes are
made. If this file and another document disagree, this file wins on
*intent* (what we are trying to do) and the specs win on *detail* (exactly
how).

Each rule has a **Status** line saying how much of it the code does today:

- **Built** — the code does this.
- **Partly built** — some of it exists; the line says what is missing.
- **Planned** — decided, but not in the code yet.
- **Open decision** — not decided yet.

Statuses were checked against the code on 2026-10-08. Update a status line
in the same pull request that changes the behavior.

## Why CT exists

CT is a private, local-first "second brain." It is built for people whose
words are the final say on their work (engineers, product managers,
authors) and for the AI agents that work for them.

You put raw material in as ordinary files. A component called the **Active
Librarian** reads those files and turns them into short, trustworthy facts
(the "wisdom layer"). Any agent, even a weak one, can then recall those
facts. The goal is that good structure does the work, so agents don't need
to be clever.

There are two ways in, both over the same pipeline:

- **The app** — friendly, for any author writing anything.
- **The agent tools** — the MCP sidecar and the `ct` command line, tuned
  for software development. In this file, "agent tools" always means both.

## Glossary

- **Vault** — the folder of real files that CT reads.
- **Brain** — CT's database of facts built from the vault.
- **Wisdom layer** — the curated facts agents recall.
- **Deposit** — an agent saving knowledge as a new file in the vault.
- **Ingest** — the Librarian reading a file and turning it into facts.
- **Reconcile** — the Librarian's upkeep: removing facts whose source file
  is gone, flagging inconsistencies, applying supersessions. (The code
  still calls part of this "heal"; see below.)
- **Tier** — how much a fact is trusted: *human* (a person stands behind
  it) or *agent*.
- **Attestation** — a record that a person confirmed a file's exact
  content.
- **Supersession** — marking an old fact as replaced by a newer one,
  instead of deleting it.
- **Recall** — looking up facts relevant to a question.
- **Injection** — facts automatically added to an agent's context at the
  start of a session (done by CTI, below).
- **ISF** — `immutable-source-files/`, the vault folder for source files.
  Agent deposits go in `immutable-source-files/agents/`.
- **CTI** — curated-thoughts-integrations, a separate repo that delivers
  wisdom into agent sessions using `ct wisdom match`.
- **System One / Jev** — a small, fast model that makes typed yes/no or
  category judgments. Jev is the one CT uses. (INTENT has also named
  "Laya"; it is not in the code.)
- **Migration 13** — the wiki-engine database change that added fact
  history columns (`valid_from`, `valid_to`, `superseded_by`); CT's copy is
  schema version V24.

## The rules

### 1. Agents never write facts directly

Agents add knowledge only by depositing a file (`wisdom_deposit`). Only the
Librarian reads those files and turns them into facts. No agent tool may
add, change, approve, or archive facts in the database. The old tools that
did this (`curated_add_wisdom`, `curated_update_wisdom`,
`curated_proposal_decide`, `curated_archive_wisdom`) are removed.

*Why:* everything in the brain should trace back to a file, and the
Librarian should be the single gatekeeper.

**Status: Partly built.** `wisdom_deposit` writes a new file and never the
facts table, and the four old tools are gone from both the MCP sidecar and
`ct`. Known gaps: `vault_write_note` can still create and edit files in the
agents deposit folder, and `ct proposals review` can approve proposals. It
refuses piped input, but an agent that opens a pseudo-terminal can get
past that check.

### 2. A fact is human-trusted only if a person vouched for it

A file earns the human tier only when the app has recorded an attestation
for its exact content. Attestations are written only by the app, never by
agent tools. Every file without one is treated as agent-written, no matter
which folder it sits in.

*Why:* CT can't tell who wrote a file by looking at it, so it requires
proof instead of guessing. Through the normal tools an agent cannot create
that proof. If one goes around them, the mismatch between the file and its
recorded origin can be detected.

When this ships, existing human files will be attested once in bulk, and
facts that were wrongly stamped as human-stated (`user_stated`) will be
re-ingested as agent facts and superseded.

**Status: Planned.** There is no attestation record in the code yet. Today
trust follows *location*: facts from the agent deposit folder get the
configured `deposit_default_tier`, and the app still writes `user_stated`
facts directly.

### 3. A deposit shows as "pending" until it has been processed

Depositing a file immediately asks for that file to be ingested. Until
ingest finishes, `wisdom_deposit_status` reports it as pending, or says
there is no ingest host if this machine can't process files. Recall and
injection never read files that haven't been ingested. The one exception
is `wisdom_pending`, a separate read-only list of unprocessed deposits,
which is never mixed into recall results.

*Why:* agents should get an honest answer ("not ready yet") rather than
half-processed knowledge.

**Status: Partly built.** The deposit kick, the status states, and
`wisdom_pending` all exist. Gap: a deposit stuck after its text is split
into chunks (but before facts are made) can still be returned by
`vault_semantic_search`.

### 4. Old facts are superseded, not deleted, and only humans overrule humans

An agent retires a fact only by depositing a supersession file
(`wisdom_propose_supersession`). If the old and new facts are both agent
tier, the Librarian applies it automatically. If the old fact is human
tier, it becomes a proposal that only a person can accept, in the app.

*Why:* history is kept, and an agent can never overwrite something a
person stands behind.

**Status: Partly built.** The tool writes the supersession file. The
Librarian does not apply supersessions yet, so they stay pending.

### 5. All of a person's machines share one set of facts

Facts sync between machines through the outbox. Each fact records which
machine ingested it, and only that machine may decide its source file is
gone. Fact IDs are computed from the file's path, its content, and the
fact's position in it, so two machines ingesting the same file produce the
same facts instead of duplicates. A machine never raises the trust tier of
a fact it received from another. Attestations sync along with the facts
they cover.

*Why:* several machines should agree on one brain without fighting over
it.

**Status: Mostly planned.** The outbox exists but only pushes one way, to
a Postgres replica; machines don't pull from each other. There is no
"ingested on" machine field, and fact IDs are random. How a machine
should trust tiers it receives from another is an **open decision**.

### 6. The fast model judges; it never writes

System One (Jev) gives quick, cheap judgments: should this file be
ingested, is this fact relevant to this question. Every judgment is logged
so it can be evaluated. It never writes facts, approves anything, rewrites
questions, or decides trust. It is optional: recall works without it.

*Why:* a small model is useful for sorting, not for deciding what is true.

**Status: Partly built, used differently.** Jev exists only as an optional
classifier that assigns types to untyped facts. It is not used for ingest
triage or recall relevance, and its judgments are not logged for
evaluation.

### 7. Recall says "I don't know" rather than guess

If the best match is weak, recall returns nothing rather than presenting a
poor answer with confidence. The cutoff is a fixed threshold set from
benchmarks. At ingest, each fact also gets embeddings for the questions it
answers, and recall uses whichever of a fact's embeddings matches best.

*Why:* a confident wrong answer is worse than no answer.

**Status: Partly built.** `ct wisdom match` has a benchmark-set cutoff
(0.70 for qwen3-embedding-4b) and refuses to answer with a model that has
no calibrated cutoff. The other recall tools (`curated_recall_context`,
wiki search) have no cutoff. Question embeddings are not built.

### 8. Facts stay inside the project they came from

Facts ingested from `works/<project>/` are only recalled within that
project. Things a person says directly are captured by the app (never by
an agent) under `stated/<author>/<year>/`, with frontmatter recording who
said it, how it was captured, and its attestation. They stay below wisdom
tier until attested.

*Why:* one project's knowledge shouldn't leak into another's answers.

**Status: Planned.** None of this is in the code yet.

### 9. Each area of the vault has its own editing rules

Agents may only add files to their deposit folder, never edit or delete
them. Corrections are new supersession deposits. People edit their own
areas freely, with git as the history. When a source file is gone, its
facts are retired, but only by the machine that ingested them.

*Why:* agent deposits become an append-only record you can audit.

**Status: Partly built.** `wisdom_deposit` only ever creates new files,
but `vault_write_note` can edit existing ones in the deposit folder (see
rule 1). Facts whose source is gone are retired by the database "heal"
pass, which is not limited to the ingesting machine.

## The Active Librarian

The Librarian has two jobs:

- **Ingest** — read files and turn them into facts.
- **Reconcile** — keep the brain healthy: retire facts whose source is
  gone, flag inconsistencies, apply supersessions.

The Librarian's repair job is called "reconcile" from now on. The code
still uses "heal" for part of it (the database heal pass and the app's
"Heal Database" button) and should be renamed over time. Separately,
`ct heal` is also the name of the *ontology* repair command (fixing entity
and edge types). That command is not part of the Librarian, and this
rename does not apply to it.

Planned Librarian improvements:

- Fact IDs computed from content, so duplicates can't appear. **Planned.**
- Question embeddings at ingest, with recall using a fact's best-matching
  embedding. **Planned.**
- Recall that skips superseded and expired facts. **Partly built:** only
  `ct wisdom match` does this today.

Design target: about 10,000 live facts per brain. Approximate
nearest-neighbor search (ANN) can wait until about 100,000.

## Out of scope (don't build these here)

- Making the agent tools general-purpose for non-software work. The app
  covers that.
- Running agent recall through the TypeScript wiki engine. Port what's
  needed into the Rust recall instead. (Today agent recall is Rust.)
- Following graph links during session-start injection (v1). Graph
  traversal stays in on-demand tools like `wiki_context`.
- Reading un-ingested files in injection or recall (see rule 3 for the
  one exception).
- Quietly ranking facts lower because of where they came from. Origin is
  recorded and shown (CTI labels it); any ranking change needs a spec and
  benchmark results.
- Deleting source files when a fact is retired.

## How changes are made

1. **Spec first.** Write a spec under `docs/superpowers/specs/`, based on
   an investigation that reads the real code (not memory). Spec, plan and
   implementation go in one pull request.
2. **Decision history** for the wisdom-deposit design lives in the
   equational-wiki vault (`wisdom-deposit-file-first-intent-2026-10-01.md`
   and its review, 2026-10-01). That vault is separate from this repo.
3. **Review, then merge.** Two independent AI reviews (GLM and Opus), CI
   green, then merge. Unresolved questions hold the pull request.
4. **Benchmarks gate behavior changes.**
   - Recall and injection changes must pass: no superseded fact is ever
     injected; LongMemEval hit rate and abstention precision; paraphrase
     tests across models (at least 200); exactly-once tests; and tests
     that cached output stays byte-identical.
   - Ingest and reconcile changes must pass the two-machine test: only
     the ingesting machine retires a fact, and ingesting the same file
     twice creates no duplicates.
   - Benchmarks run on scratch brains with real embeddings. Fake
     embeddings are only for unit tests.
5. **Never touch the live brain** outside the app. Tests use scratch
   profiles.
