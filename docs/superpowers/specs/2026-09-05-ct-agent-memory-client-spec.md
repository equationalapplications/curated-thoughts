# Spec: Curated Thoughts as the agent memory backend (Hermes/Tessera client spec)

**Status:** Approved design, rides PR #184 (spec phase).
**Date:** 2026-09-05
**Baseline:** CT `main` @ `2bf1c189` (v2.4.3); Hermes config as deployed on
the T420 (CT sidecar already registered).
**Scope:** How Equational Applications' AI agents connect to, read from, and
(one PR later) write to Curated Thoughts as their shared memory layer.
Companion to `specs/curated-thoughts-mcp-coding-spec.md` (the coding-agent
server spec) — that spec covers Aider/VS Code Copilot; this one covers the
Hermes-resident agents (Tessera, subagents, cron jobs).

## §1 — Why this spec exists

EA agents already depend on CT as their memory:

- Tessera's durable memories live in the equational-wiki vault, which IS the
  CT vault (`vault_path` in `~/.brain/config.json` points at
  `~/Documents/equational-wiki`).
- Hermes spawns a CT MCP sidecar automatically; every Tessera session inherits
  `mcp__curated_thoughts__*` tools.
- Yet no document specifies the client contract: which server binary, which
  tool surface, what is allowed where, and what agents must never do.

This spec is that contract. It makes the existing wiring explicit and defines
the write-path rules that PR #184's implementation will enable.

## §2 — Server selection (which binary an agent attaches to)

| Server | Binary | Tools | Intended client |
|---|---|---|---|
| Main vault/wiki server | `/usr/bin/curated-thoughts --mcp` | 8 (`vault_*`, `wiki_*`) + the six `curated_*` after PR #184 implements | Hermes-resident agents (Tessera, subagents, cron) |
| Coding-focused server | `curated-thoughts-mcp` (tools crate) | 6 `curated_*`/`vault_*` + `graph_neighbors`, `curated_superpowers_setup` | External coding agents (Aider, VS Code Copilot) |

Rule: Hermes agents use the **main server** (already configured under
`mcp_servers.curated-thoughts` in `~/.hermes/config.yaml` with command
`/usr/bin/curated-thoughts-mcp --mcp`). They do NOT spawn a second server via
the tools crate — that would open a second connection to the same brain and
duplicate the tool surface. The coding server remains for external editors.

Note on a long-standing naming trap: the installed sidecar command and the
tools-crate binary are BOTH named `curated-thoughts-mcp`. Disambiguate by
path: `/usr/bin/curated-thoughts-mcp` is the main server (a packaged variant
of the main binary); the tools-crate build lives under the repo's
`target/` and is for coding agents.

## §3 — Tool contract after PR #184 lands

Read surface (already live):

- `vault_semantic_search`, `vault_related_chunks` — vault RAG.
- `wiki_search`, `wiki_context`, `wiki_get_ontology`, `wiki_traverse_graph`
  — the llm-wiki graph (Active Librarian layer).
- `vault_write_note`, `vault_upsert_index_entry` — vault file writes (OKF).

Memory surface (new with PR #184):

- `curated_recall_context(query, limit_wiki?, limit_code?)` — pre-task
  context: wiki entries + code chunks ranked together.
- `curated_get_wiki_entry(topic? | entity_id?)` — full entry body; if both
  given, `entity_id` wins (precedence stated in the tool description).
- `curated_search_code(query, limit?, symbol?)` — ast code-chunk search.
- `curated_add_wisdom(entity_id, body)` — persist a learned fact
  (`source_type='user_stated'`, audited outbox path).
- `curated_update_wisdom(entity_id, fact_id, body)` — rewrite a fact.
- `curated_archive_wisdom(entity_id, fact_id)` — soft-delete a fact.

Contract details (error behavior, access-log semantics, RW-connection rules)
are specified in
`docs/superpowers/specs/2026-09-05-agent-memory-crud-mcp-tools-design.md`
§5–§7 and apply equally to every client.

## §4 — Agent usage policy (the rules agents must follow)

1. **Session start**: agents doing vault-adjacent work SHOULD prime context
   via `wiki_context`/`curated_recall_context` rather than re-deriving from
   files.
2. **Task end**: durable lessons SHOULD be deposited via
   `curated_add_wisdom` (once available) or `vault_write_note` — never left
   only in session context.
3. **Write routing**: vault *files* → `vault_write_note` (OKF frontmatter
   required). Brain *wiki facts* → `curated_add_wisdom`. Never raw-SQL the
   brain, never write `~/.brain/*` files directly.
4. **Attachment rule**: agents attach ONLY to the main server (§2). Spawning
   the coding server on this machine is reserved for coding-agent sessions
   and requires the user's explicit ask.
5. **Fail-closed**: if the sidecar is unreachable (CT tool calls erroring),
   agents must NOT fall back to writing brain state through other means; they
   report the outage and continue degraded (vault file notes remain allowed —
   they are files, not brain rows).

## §5 — Testing / verification

- Sidecar health: an initialize + `tools/list` handshake over stdio returns
  the §3 tool list (this is how the v2.4.3 install was verified).
- `curated_*` calls succeed against the live brain only when the brain DB is
  readable; integration coverage is defined in the CRUD spec §9 and shared
  here by reference.

## §6 — Docs

After implementation merges: the CT product page's MCP section gains a line
pointing at this spec as the agent-client contract, and
`specs/curated-thoughts-mcp-coding-spec.md` gets a cross-reference so the two
specs explicitly split the client population (Hermes agents vs coding
agents).
