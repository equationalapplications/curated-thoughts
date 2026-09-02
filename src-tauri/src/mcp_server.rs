//! MCP stdio server for vault search. Activated when the binary is launched with `--mcp`.

use std::sync::Arc;

use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ServiceExt};
use tracing::dispatcher::Dispatch;

use crate::retrieval;
use crate::tool_dispatch::{self, ToolDispatchContext};

#[derive(Clone)]
struct VaultMcpServer {
    ctx: ToolDispatchContext,
}

#[tool_router(server_handler)]
impl VaultMcpServer {
    #[tool(
        name = "vault_semantic_search",
        description = "Semantic search over vault chunks using the configured embedding profile."
    )]
    async fn vault_semantic_search(
        &self,
        args: Parameters<tool_dispatch::VaultSemanticSearchParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(params) = args;
        let value = serde_json::to_value(params)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("params encode: {e}"), None))?;
        let result = tool_dispatch::dispatch_tool_call(&self.ctx, "vault_semantic_search", value)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(retrieval::mcp_error_hint(&e), None))?;
        serde_json::to_string(&result)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }

    #[tool(
        name = "vault_related_chunks",
        description = "List chunks related to a vault document path. Accepts vault-relative paths (e.g. `notes/meeting.md`) or absolute paths — tries multiple path spellings for maximum compatibility."
    )]
    async fn vault_related_chunks(
        &self,
        args: Parameters<tool_dispatch::VaultRelatedChunksParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(params) = args;
        let value = serde_json::to_value(params)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("params encode: {e}"), None))?;
        let result = tool_dispatch::dispatch_tool_call(&self.ctx, "vault_related_chunks", value)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(retrieval::mcp_error_hint(&e), None))?;
        serde_json::to_string(&result)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }

    #[tool(
        name = "wiki_search",
        description = "Semantic search over llm_wiki_entries (Active Librarian facts). Returns entry ids for use with wiki_traverse_graph, each with its stored tier. Optional tier filter: \"fact\" or \"wisdom\"; omit for every live entry."
    )]
    async fn wiki_search(
        &self,
        args: Parameters<tool_dispatch::WikiSearchParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(params) = args;
        let value = serde_json::to_value(params)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("params encode: {e}"), None))?;
        let result = tool_dispatch::dispatch_tool_call(&self.ctx, "wiki_search", value)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(retrieval::mcp_error_hint(&e), None))?;
        serde_json::to_string(&result)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }

    #[tool(
        name = "wiki_context",
        description = "One call: semantic search over wiki facts PLUS the graph neighborhood around what it finds. Returns {facts, entities, edges, provenance, truncated}. Prefer this over wiki_search + wiki_traverse_graph — it needs no entity-id or namespace knowledge. Params: query (required); maxFacts (default 5) seed facts; depth (default 1, clamped to 3); optional tier filter \"fact\" or \"wisdom\". An unlinked corpus returns edges: [] rather than an error."
    )]
    async fn wiki_context(
        &self,
        args: Parameters<tool_dispatch::WikiContextParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(params) = args;
        let value = serde_json::to_value(params)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("params encode: {e}"), None))?;
        let result = tool_dispatch::dispatch_tool_call(&self.ctx, "wiki_context", value)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(retrieval::mcp_error_hint(&e), None))?;
        serde_json::to_string(&result)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }

    #[tool(
        name = "wiki_get_ontology",
        description = "Return the ontology manifest (node_types, edge_types) for a wiki entity tier."
    )]
    async fn wiki_get_ontology(
        &self,
        args: Parameters<tool_dispatch::WikiGetOntologyParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(params) = args;
        let value = serde_json::to_value(params)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("params encode: {e}"), None))?;
        let result = tool_dispatch::dispatch_tool_call(&self.ctx, "wiki_get_ontology", value)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(retrieval::mcp_error_hint(&e), None))?;
        serde_json::to_string(&result)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }

    #[tool(
        name = "wiki_traverse_graph",
        description = "BFS traversal of llm_wiki_edges from a source entry id. Use wiki_search first to obtain sourceId."
    )]
    async fn wiki_traverse_graph(
        &self,
        args: Parameters<tool_dispatch::WikiTraverseGraphParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(params) = args;
        let value = serde_json::to_value(params)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("params encode: {e}"), None))?;
        let result = tool_dispatch::dispatch_tool_call(&self.ctx, "wiki_traverse_graph", value)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(retrieval::mcp_error_hint(&e), None))?;
        serde_json::to_string(&result)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }

    #[tool(
        name = "vault_write_note",
        description = "Write or update a markdown note with OKF v0.1 frontmatter. Path safety: must be under vault root. If-Match semantics: on edits, frontmatter.updated_at must EXACTLY match the file\'s current updated_at token (mtime is never consulted); mismatch returns stale_update:{current}. On create the tool stamps a fresh token for you. Atomic write via temp file + rename."
    )]
    async fn vault_write_note(
        &self,
        args: Parameters<tool_dispatch::VaultWriteNoteParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(params) = args;
        let value = serde_json::to_value(params)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("params encode: {e}"), None))?;
        let result = tool_dispatch::dispatch_tool_call(&self.ctx, "vault_write_note", value)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(retrieval::mcp_error_hint(&e), None))?;
        serde_json::to_string(&result)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }

    #[tool(
        name = "vault_upsert_index_entry",
        description = "Atomically upsert an entry into an EXISTING markdown index file (never auto-created; missing index returns index_not_found). Entry names: letters/digits/spaces/_/-/. only, matched by whole-line equality against '## {entry_name}' (no regex). Replaces the block through the next '## ' header or EOF; appends if absent. entry_path must exist in the vault. Atomic write via temp file + rename; repeated calls are idempotent."
    )]
    async fn vault_upsert_index_entry(
        &self,
        args: Parameters<tool_dispatch::VaultUpsertIndexEntryParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(params) = args;
        let value = serde_json::to_value(params)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("params encode: {e}"), None))?;
        let result =
            tool_dispatch::dispatch_tool_call(&self.ctx, "vault_upsert_index_entry", value)
                .await
                .map_err(|e| {
                    rmcp::ErrorData::internal_error(retrieval::mcp_error_hint(&e), None)
                })?;
        serde_json::to_string(&result)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }
}

/// Blocking entrypoint for `--mcp` mode. Calls into a tokio runtime internally.
/// All tracing/logging must go to stderr only — stdout carries JSON-RPC frames.
pub fn run() -> anyhow::Result<()> {
    let subscriber = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .finish();
    let dispatcher = Dispatch::new(subscriber);
    if let Err(err) = tracing::dispatcher::set_global_default(dispatcher.clone()) {
        eprintln!("curated-thoughts [--mcp]: failed to set global tracing subscriber: {err}");
    }
    let _subscriber_guard = tracing::dispatcher::set_default(&dispatcher);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async_run())
}

async fn async_run() -> anyhow::Result<()> {
    let p = retrieval::resolve_brain_paths();

    let profile = retrieval::load_embed_profile(&p.config_path).map_err(|e| {
        eprintln!(
            "curated-thoughts [--mcp]: failed to load embed profile from {}: {e}",
            p.config_path.display()
        );
        e
    })?;

    let conn = retrieval::open_brain_readonly(&p.db_path).map_err(|e| {
        eprintln!("curated-thoughts [--mcp]: {e}");
        e
    })?;

    let vault_dir = crate::vault::VaultConfig::new(p.config_path.clone())
        .get_vault_path()
        .ok()
        .flatten()
        .map(std::path::PathBuf::from)
        .and_then(|path| path.canonicalize().ok());

    let server = VaultMcpServer {
        ctx: ToolDispatchContext {
            conn: Arc::new(std::sync::Mutex::new(conn)),
            profile,
            vault_dir,
            client: "local-mcp".into(), // static label; actual client name only known after initialize handshake
        },
    };

    let transport = rmcp::transport::stdio();
    let handle = server
        .serve(transport)
        .await
        .map_err(|e| anyhow::anyhow!("MCP server failed to start: {e}"))?;
    handle
        .waiting()
        .await
        .map_err(|e| anyhow::anyhow!("MCP server task ended with error: {e}"))?;
    Ok(())
}
