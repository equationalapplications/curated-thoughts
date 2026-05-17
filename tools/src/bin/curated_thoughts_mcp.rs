//! MCP stdio server for vault search (`rmcp`).

use std::sync::{Arc, Mutex, MutexGuard};

use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ServiceExt};
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::Deserialize;

use tauri_app_lib::embedder::EmbedProfile;
use tauri_app_lib::retrieval;

#[derive(Clone)]
struct VaultMcpServer {
    conn: Arc<Mutex<Connection>>,
    profile: EmbedProfile,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct VaultSemanticSearchParams {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct VaultRelatedChunksParams {
    doc_path: String,
    #[serde(default)]
    limit: Option<usize>,
}

fn lock_conn(conn: &Arc<Mutex<Connection>>) -> Result<MutexGuard<'_, Connection>, rmcp::ErrorData> {
    conn.lock()
        .map_err(|_| rmcp::ErrorData::internal_error("database mutex poisoned", None))
}

#[tool_router(server_handler)]
impl VaultMcpServer {
    #[tool(
        name = "vault_semantic_search",
        description = "Semantic search over vault chunks using the configured embedding profile."
    )]
    async fn vault_semantic_search(
        &self,
        args: Parameters<VaultSemanticSearchParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(VaultSemanticSearchParams { query, limit }) = args;
        let limit = limit.unwrap_or(10);
        let conn = lock_conn(&self.conn)?;
        let hits = retrieval::semantic_search_chunks(&conn, &self.profile, &query, limit)
            .map_err(|e| rmcp::ErrorData::internal_error(retrieval::mcp_error_hint(&e), None))?;
        serde_json::to_string(&hits)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }

    #[tool(
        name = "vault_related_chunks",
        description = "List chunks related to a vault document path."
    )]
    async fn vault_related_chunks(
        &self,
        args: Parameters<VaultRelatedChunksParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(VaultRelatedChunksParams { doc_path, limit }) = args;
        let limit = limit.unwrap_or(5);
        let conn = lock_conn(&self.conn)?;
        let hits = retrieval::related_chunks_facade(&conn, &doc_path, limit)
            .map_err(|e| rmcp::ErrorData::internal_error(retrieval::mcp_error_hint(&e), None))?;
        serde_json::to_string(&hits)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let p = retrieval::resolve_brain_paths();
    let profile = retrieval::load_embed_profile(&p.config_path).map_err(|e| {
        eprintln!(
            "curated-thoughts-mcp: failed to load embed profile from {}: {e}",
            p.config_path.display()
        );
        e
    })?;
    let conn = retrieval::open_brain_readonly(&p.db_path).map_err(|e| {
        eprintln!("curated-thoughts-mcp: {}", e);
        e
    })?;

    fn configured_database_url() -> Option<String> {
        let db_url = std::env::var("DATABASE_URL").ok()?;
        let db_url = db_url.trim();
        if db_url.is_empty() {
            None
        } else {
            Some(db_url.to_string())
        }
    }

    if let Some(db_url) = configured_database_url() {
        let config = tauri_app_lib::outbox::OutboxConfig {
            sqlite_path: p.db_path.clone(),
            db_url,
            ..tauri_app_lib::outbox::OutboxConfig::default()
        };
        let _ = tauri_app_lib::outbox::postgres::spawn_postgres_worker(config, None);
    }

    let server = VaultMcpServer {
        conn: Arc::new(Mutex::new(conn)),
        profile,
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
