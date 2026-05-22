# Curated Thoughts Coding-Focused MCP Server Specification

## 1. Document Purpose
This specification defines the behavior, tooling, and Superpowers framework alignment of the Curated Thoughts coding-focused Model Context Protocol (MCP) server. This server is designed to provide coding agents (Aider, VS Code Copilot) with persistent context retrieval, wisdom layer management, and code search capabilities, fully integrated with the Superpowers agentic skills framework.

This spec is informed by the official Curated Thoughts Superpowers skill file located at `.skills/curated-thoughts/skill.md`, which defines agent-facing tool usage guidelines and workflow rules.

---

## 2. Server Overview
- **Server Name**: `curated-thoughts-mcp` (binary name, matches `[[bin]]` entry in `tools/Cargo.toml`)
- **Primary Use Case**: Coding-focused context augmentation for AI agents, prioritizing the Curated Thoughts wisdom layer (wiki) and code chunk repository for software development tasks.
- **Protocol**: MCP stdio transport via the `rmcp` Rust crate.
- **Superpowers Alignment**: Fully compliant with the Curated Thoughts Superpowers skill, supporting all defined workflows and tool contracts.

---

## 3. Superpowers Framework Alignment
The server adheres to all guidelines defined in `.skills/curated-thoughts/skill.md`:
1. **Tool Availability**: Exposes all 7 tools defined in the Superpowers skill file (see Section 4).
2. **Workflow Support**:
   - Pre-task context recall via `curated_recall_context`
   - Code modification support via `curated_search_code`
   - Post-task wisdom persistence via `curated_add_wisdom`
   - Native compatibility with Superpowers brainstorming, TDD, and other agentic workflows
3. **Setup Compatibility**: Provides the `curated_superpowers_setup` tool to generate step-by-step instructions for configuring the server with Aider and VS Code Copilot Superpowers integrations.

---

## 4. MCP Tool Specification
All tools are exposed via the `curated-thoughts` MCP server identifier, matching the Superpowers skill definition:

### 4.1 `curated_recall_context`
- **Description**: Recall prioritized context from the Curated Thoughts wisdom layer (wiki) and vault code chunks for a coding task. Returns wiki entries first, then relevant code chunks, all ranked by relevance to the query.
- **Parameters**:
  - `query` (String, required): Coding task query to recall context for
  - `limit_wiki` (usize, optional, default: 5): Max number of wisdom layer (wiki) entries to return
  - `limit_code` (usize, optional, default: 10): Max number of code chunks to return
- **Returns**: JSON object with `wiki_entries` (array of ranked wiki chunks), `code_chunks` (array of ranked code chunks), `query` (original query)

### 4.2 `curated_search_code`
- **Description**: Search Curated Thoughts code chunks (CodeLike strategy) for a query or symbol, returning relevant code snippets for coding tasks.
- **Parameters**:
  - `query` (String, required): Query to search code chunks
  - `limit` (usize, optional, default: 10): Max number of code chunks to return
  - `symbol` (String, optional): Optional symbol name to filter code chunks (e.g., function name)
- **Returns**: JSON object with `code_chunks` (array of ranked code chunks with metadata), `query`, `symbol_filter`

### 4.3 `curated_get_wiki_entry`
- **Description**: Fetch full content of a specific Curated Thoughts wiki (wisdom layer) entry by topic or entity ID.
- **Parameters**:
  - `topic` (String, optional): Topic to search for in wiki entries (matches document path)
  - `entity_id` (String, optional): Specific entity ID of the wiki entry to fetch
  - *Note: Either `topic` or `entity_id` must be provided*
- **Returns**: JSON object with `full_text` (concatenated wiki entry text), `chunks` (array of individual chunks with metadata), `topic`, `entity_id`

### 4.4 `curated_add_wisdom`
- **Description**: Add new entries to the Curated Thoughts wisdom layer for future recall, to persist coding patterns and solutions.
- **Parameters**:
  - `topic` (String, required): Topic/path for the wiki entry (document path in tier 'wiki')
  - `text` (String, required): Full text content of the wisdom entry to add
  - `entity_id` (String, optional): Optional entity ID to associate with the chunk
  - `symbol` (String, optional): Optional symbol associated with the wisdom (e.g., function name)
  - `language` (String, optional): Optional language of the content (e.g., "rust", "typescript")
- **Returns**: JSON object with `success` (boolean), `doc_id`, `chunk_id`, `topic`, `message`

### 4.5 `vault_semantic_search`
- **Description**: Semantic search over all vault chunks using the configured embedding profile.
- **Parameters**:
  - `query` (String, required): Search query
  - `limit` (usize, optional, default: 10): Max number of results to return
- **Returns**: JSON array of semantic search hits with metadata

### 4.6 `vault_related_chunks`
- **Description**: List chunks related to a specific vault document path.
- **Parameters**:
  - `doc_path` (String, required): Vault document path to find related chunks for
  - `limit` (usize, optional, default: 5): Max number of results to return
- **Returns**: JSON array of related chunks with metadata

### 4.7 `curated_superpowers_setup`
- **Description**: Get step-by-step instructions to set up the Superpowers agentic skills framework for Aider and VS Code Copilot, integrated with Curated Thoughts MCP tools.
- **Parameters**:
  - `include_aider` (boolean, optional, default: true): Include Aider setup instructions
  - `include_vscode` (boolean, optional, default: true): Include VS Code Copilot setup instructions
- **Returns**: JSON object with `instructions` (markdown formatted setup steps), `setup_complete` (boolean), `next_step`

---

## 5. Technical Implementation Details
### 5.1 Dependencies
Defined in `tools/Cargo.toml`:
- `tauri_app_lib`: Local path dependency to the main Curated Thoughts Tauri app library, provides embedding, search, and retrieval utilities
- `rmcp` (v1.6+): MCP server framework with macro support, schemars integration, and stdio transport
- `rusqlite` (v0.31+): SQLite database access with bundled SQLite
- `sha2` (v0.10+): SHA-256 hashing for wisdom entry versioning
- `tokio` (v1+): Async runtime for MCP server operation
- `serde`/`serde_json`: Serialization/deserialization for tool parameters and return values
- `schemars`: JSON schema generation for tool parameter validation

### 5.2 Database Access
- Default connection: Read-only SQLite connection to the Curated Thoughts brain database for search tools
- Write access: Enabled for `curated_add_wisdom` tool to insert new wiki documents, chunks, and embeddings
- Thread safety: `Arc<Mutex<Connection>>` wraps the SQLite connection to ensure safe concurrent access from async MCP tool handlers

### 5.3 Embedding & Ranking
- Embedding generation: Uses `tauri_app_lib::embedder::embed_batch` with the configured `EmbedProfile` (loaded from Curated Thoughts config)
- Similarity ranking: Cosine similarity via `tauri_app_lib::search::cosine_similarity` to rank wiki and code chunks by relevance to query embeddings
- Embedding storage: Chunks store embeddings as raw byte arrays, converted to `f32` vectors via `bytes_to_f32` for similarity calculation

---

## 6. Coding Workflow Integration
The server is designed to integrate seamlessly with Superpowers coding workflows, as defined in the skill file:

### 6.1 Pre-Task Workflow
For any new coding task, agents must first call `curated_recall_context` with the task description to fetch relevant wisdom layer entries and existing code patterns. Example agent prompt:
> "Recall context for adding a Rust MCP tool with error handling"

### 6.2 Code Modification Workflow
When modifying existing code, agents call `curated_search_code` with the target symbol or query to retrieve related implementations. Example:
> "Search code for function `lock_conn`"

### 6.3 Post-Task Workflow
After completing non-trivial coding tasks, agents call `curated_add_wisdom` to persist new patterns to the wisdom layer for future recall. Example:
> "Add wisdom entry for topic `rust-mcp-error-handling` with text describing the new error handling pattern used in `curated_thoughts_mcp.rs`"

### 6.4 Superpowers Workflow Combination
Agents can combine Superpowers native workflows (brainstorming, TDD) with Curated Thoughts context. Example:
> "Run the Superpowers TDD workflow for the new module, using `curated_recall_context` to fetch existing test patterns."

---

## 7. Setup & Configuration
### 7.1 Superpowers Setup
Run the `curated_superpowers_setup` MCP tool to generate tailored instructions for:
- Aider: Global OpenSkills install, Superpowers sync, `.aider.conf.yml` configuration
- VS Code Copilot: `.vscode/mcp.json` configuration, Copilot Chat integration

### 7.2 Aider Configuration
Example `.aider.conf.yml` (auto-generated via setup tool):
