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
| `curated_add_wisdom` | Add new entries to the wisdom layer for future recall, to persist coding patterns and solutions. |
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
Call `curated_add_wisdom` to save new patterns to the wisdom layer. Example:
> "Add wisdom entry for topic `typescript-api-error-handling` with text describing the new error handling pattern."

### Using Superpowers Workflows
Combine Superpowers workflows (brainstorming, TDD, etc.) with Curated Thoughts context:
> "Run the Superpowers TDD workflow for the new module, using `curated_recall_context` to fetch existing test patterns."

## Setup
Run the `curated_superpowers_setup` MCP tool to get detailed step-by-step instructions for setting up Superpowers with Aider and VS Code Copilot.
