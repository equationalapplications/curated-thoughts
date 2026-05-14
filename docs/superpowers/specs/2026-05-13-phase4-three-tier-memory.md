## 🛠️ Phase 4: Validation, Maintenance, and Hardening

### 1. Integration & "Impact Radius" Validation

This is the final check of the [Code Graph](https://github.com/equationalapplications/curated-thoughts#bulk-re-index-bulk_reindex-cli) logic. You use the [MCP agent server](https://github.com/equationalapplications/curated-thoughts#mcp-agent-server-experimental) to verify that a change in a **Fact** (e.g., an API spec) correctly identifies all affected **Working Memory** (code chunks) up to 5 levels deep.

* **Tooling:** Run the [integration tests](https://www.google.com/search?q=https://github.com/equationalapplications/curated-thoughts%23integration-tests-stdio--vault_tools) to verify that the Rust backend's recursive CTE is tracing dependencies correctly.
* **Goal:** Ensure the [Librarian](https://github.com/equationalapplications/curated-thoughts#architecture) can cite exact file and line references when flagging architectural inconsistencies.

### 2. Database "Heal" and "Prune" Automation

In Phase 4, you move from manual controls to automated [background ingestion pipeline](https://github.com/equationalapplications/curated-thoughts#architecture) maintenance.

* **Auto-Heal:** The [file watcher](https://github.com/equationalapplications/curated-thoughts#architecture) triggers a debounced `runHeal` to remove "ghost notes" (references to deleted files).
* **Immutable fact protection:** `runHeal` and the prune pipeline must never remove `immutable_document` facts; only disconnected or stale `librarian_inferred` entries may be cleaned up.
* **Hard-Pruning:** The system automatically scrubs soft-deleted notes from the SQLite `brain.db` after 7 days to manage disk space and performance.

### 3. Semantic Search Profiling

As your knowledge base grows, local [semantic search profiling](https://github.com/equationalapplications/curated-thoughts#semantic-search-profiling) becomes critical. You test query latency against high chunk counts to determine if you need to migrate from standard cosine similarity to native extensions like `sqlite-vec`.

### 4. Production Hardening

* **Vector Cache Management:** Implement strict caps on memory usage (500 vectors per entity) to ensure [Curated Thoughts](https://github.com/equationalapplications/curated-thoughts) remains lightweight on desktop hardware.
* **Security Scans:** Verify that [Source Normalization](https://github.com/equationalapplications/curated-thoughts#security) is preventing path injection and that the [MCP server](https://github.com/equationalapplications/curated-thoughts#mcp-agent-server-experimental) respects your local trust boundaries.

---

### Summary of the 4-Phase Rollout

| Phase | Focus | Key Output |
| --- | --- | --- |
| **Phase 1** | **Hierarchy** | [Tiered Weights](https://github.com/equationalapplications/curated-thoughts#architecture) (Fact vs. Working). |
| **Phase 2** | **Structure** | [Code Graph](https://github.com/equationalapplications/curated-thoughts#bulk-re-index-bulk_reindex-cli) (Symbol & Edge extraction). |
| **Phase 3** | **UX & Status** | [Reactive Hooks](https://github.com/equationalapplications/curated-thoughts#package-links) & Emoji-safe rendering. |
| **Phase 4** | **Validation** | [Integration Tests](https://www.google.com/search?q=https://github.com/equationalapplications/curated-thoughts%23integration-tests-stdio--vault_tools) & Maintenance Automation. |

With Phase 4 complete, your [Curated Thoughts](https://github.com/equationalapplications/curated-thoughts) instance is a fully autonomous, structurally aware "Second Brain."