# Curated Thoughts Integration Skill

This skill enables Superpowers agents (Aider, VS Code Copilot, etc.) to leverage Curated Thoughts MCP tools for context-aware coding tasks.

## Overview
Curated Thoughts provides a persistent wisdom layer (wiki) and code chunk search, exposed via an MCP server. This skill documents how to use these tools within Superpowers workflows.

## Available MCP Tools
All tools are exposed via the `curated-thoughts` MCP server:
| Tool Name | Description |
|-----------|-------------|
| `curated_recall_context` | Recall prioritized context from the wisdom layer (wiki) and vault code chunks for a coding task. Returns wiki entries first, then code chunks ranked by relevance. |
| `curated_search_code` | Search code chunks (CodeLike strategy) by query or symbol, returning relevant snippets for coding tasks. |
| `curated_get_wiki_entry` | Fetch full content of a specific wiki (wisdom layer) entry by topic or entity ID. |
| `wisdom_deposit` | Append-only deposit of a fact file under `immutable-source-files/agents/` (the sanctioned agent write path; INTENT rule 1). The host ingests and the Librarian processes it; check progress with `wisdom_deposit_status`. |
| `wisdom_deposit_status` | Ingest state of one deposit (path-keyed, librarian-evidence-backed). |
| `wisdom_propose_supersession` | Propose superseding an existing fact (writes a supersession deposit under `agents/supersessions/`; never deletes). |
| `wisdom_pending` | List deposits without librarian evidence (read-only; never surfaces in recall). |
| `vault_semantic_search` | Semantic search over all vault chunks using the configured embedding profile. |
| `vault_related_chunks` | List chunks related to a specific vault document path. |
| `curated_superpowers_setup` | Get step-by-step setup instructions for Superpowers with Aider and VS Code Copilot. |

## Workflow Guidelines
### Before Starting Any Coding Task
Call `curated_recall_context` with the task description to fetch relevant wisdom and code patterns. Example:
> "Recall context for adding a TypeScript API endpoint with error handling"

### When Modifying Existing Code
Call `curated_search_code` with the symbol name or query to find related implementations. Example:
> "Search code for function `handleApiRequest`"

### After Completing Non-Trivial Tasks
Call `wisdom_deposit` to append a fact file to the wisdom layer. The deposit is written with agent provenance; the host ingests it and the Librarian turns it into wisdom. Example:
> "Deposit wisdom for topic `typescript-api-error-handling` with text describing the new error handling pattern."

Note: there is no edit or delete path — corrections are new `wisdom_deposit`s (or `wisdom_propose_supersession` against an existing fact's source_ref). Direct row insertion tools (`curated_add_wisdom` & co.) were removed per INTENT rule 1.

### Using Superpowers Workflows
Combine Superpowers workflows (brainstorming, TDD, etc.) with Curated Thoughts context:
> "Run the Superpowers TDD workflow for the new module, using `curated_recall_context` to fetch existing test patterns."

## Setup
Run the `curated_superpowers_setup` MCP tool to get detailed step-by-step instructions for setting up Superpowers with Aider and VS Code Copilot.
