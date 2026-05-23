//! MCP stdio server for vault search. Activated when the binary is launched with `--mcp`.

use std::sync::{Arc, Mutex, MutexGuard};

use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ServiceExt};
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::embedder::EmbedProfile;
use crate::retrieval;

#[derive(Clone)]
struct VaultMcpServer {
    conn: Arc<Mutex<Connection>>,
    profile: EmbedProfile,
    brain_dir: std::path::PathBuf,
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

fn lock_conn(
    conn: &Arc<Mutex<Connection>>,
) -> Result<MutexGuard<'_, Connection>, rmcp::ErrorData> {
    conn.lock()
        .map_err(|_| rmcp::ErrorData::internal_error("database mutex poisoned", None))
}

fn normalize_vault_path(doc_path: &str, brain_dir: &std::path::Path) -> String {
    let p = std::path::Path::new(doc_path);
    if p.is_absolute() {
        if let Ok(rel) = p.strip_prefix(brain_dir) {
            if !rel.as_os_str().is_empty() {
                return rel.to_string_lossy().into_owned();
            }
        }
    }
    doc_path.to_string()
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
        description = "List chunks related to a vault document path. Accepts both relative and absolute paths."
    )]
    async fn vault_related_chunks(
        &self,
        args: Parameters<VaultRelatedChunksParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(VaultRelatedChunksParams { doc_path, limit }) = args;
        let limit = limit.unwrap_or(5);
        let normalized = normalize_vault_path(&doc_path, &self.brain_dir);
        let conn = lock_conn(&self.conn)?;
        let hits = retrieval::related_chunks_facade(&conn, &normalized, limit)
            .map_err(|e| rmcp::ErrorData::internal_error(retrieval::mcp_error_hint(&e), None))?;
        serde_json::to_string(&hits)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }
}

/// Blocking entrypoint for `--mcp` mode. Calls into a tokio runtime internally.
/// All tracing/logging must go to stderr only — stdout carries JSON-RPC frames.
pub fn run() -> anyhow::Result<()> {
    // Redirect all tracing to stderr so it never corrupts the JSON-RPC stream.
    let subscriber = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);

    let rt = tokio::runtime::Builder::new_current_thread()
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

    if let Some(db_url) = configured_database_url() {
        let config = crate::outbox::OutboxConfig {
            sqlite_path: p.db_path.clone(),
            db_url,
            ..crate::outbox::OutboxConfig::default()
        };
        let _ = crate::outbox::postgres::spawn_postgres_worker(config, None);
    }

    let server = VaultMcpServer {
        conn: Arc::new(Mutex::new(conn)),
        profile,
        brain_dir: p.brain_dir.clone(),
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

fn configured_database_url() -> Option<String> {
    let db_url = std::env::var("DATABASE_URL").ok()?;
    let db_url = db_url.trim();
    if db_url.is_empty() {
        None
    } else {
        Some(db_url.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_vault_path;

    #[test]
    fn strips_brain_dir_prefix_from_absolute_path() {
        let brain = std::path::Path::new("/home/user/.brain");
        assert_eq!(
            normalize_vault_path("/home/user/.brain/notes/meeting.md", brain),
            "notes/meeting.md"
        );
    }

    #[test]
    fn passthrough_for_relative_path() {
        let brain = std::path::Path::new("/home/user/.brain");
        assert_eq!(
            normalize_vault_path("notes/meeting.md", brain),
            "notes/meeting.md"
        );
    }

    #[test]
    fn passthrough_when_outside_brain_dir() {
        let brain = std::path::Path::new("/home/user/.brain");
        assert_eq!(
            normalize_vault_path("/tmp/other/file.md", brain),
            "/tmp/other/file.md"
        );
    }

    #[test]
    fn passthrough_when_path_equals_brain_dir() {
        let brain = std::path::Path::new("/home/user/.brain");
        assert_eq!(
            normalize_vault_path("/home/user/.brain", brain),
            "/home/user/.brain"
        );
    }
}
