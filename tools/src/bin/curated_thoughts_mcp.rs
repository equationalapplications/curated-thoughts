//! MCP stdio server for vault search (`rmcp`).

use std::sync::{Arc, Mutex, MutexGuard};

use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ServiceExt};
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::Deserialize;

use curated_thoughts_tools::cli_common::{
    fetch_ranked_chunks, rank_wiki_entries as shared_rank_wiki_entries, resolve_symbol,
    RECALL_CHUNKS_AST_FILTER, RECALL_CHUNKS_SQL_BASE,
};
use tauri_app_lib::embedder::{embed_batch, EmbedProfile};
use tauri_app_lib::retrieval;

fn rmcp_internal(e: anyhow::Error) -> rmcp::ErrorData {
    rmcp::ErrorData::internal_error(e.to_string(), None)
}

#[derive(Clone)]
struct VaultMcpServer {
    conn: Arc<Mutex<Connection>>,
    /// Separate read-write connection for best-effort agent-access logging;
    /// the primary connection is read-only, so INSERTs on it always fail.
    log_conn: Arc<Mutex<Connection>>,
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

#[derive(Debug, Deserialize, JsonSchema)]
struct CuratedRecallContextParams {
    /// Coding task query to recall context for
    query: String,
    /// Max number of wisdom layer (wiki) entries to return (default: 5)
    #[serde(default)]
    limit_wiki: Option<usize>,
    /// Max number of code chunks to return (default: 10)
    #[serde(default)]
    limit_code: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CuratedGetWikiEntryParams {
    /// Topic to search for in wiki entries (matches document path)
    #[serde(default)]
    topic: Option<String>,
    /// Specific entity ID of the wiki entry to fetch
    #[serde(default)]
    entity_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CuratedSearchCodeParams {
    /// Query to search code chunks
    query: String,
    /// Max number of code chunks to return (default: 10)
    #[serde(default)]
    limit: Option<usize>,
    /// Optional symbol name to filter code chunks (e.g., function name)
    #[serde(default)]
    symbol: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GraphNeighborsParams {
    /// Chunk ID of the root symbol (from curated_search_code results)
    #[serde(default)]
    chunk_id: Option<i64>,
    /// Symbol name to resolve to the root chunk (e.g., function name). Used when chunk_id is omitted.
    #[serde(default)]
    symbol: Option<String>,
    /// Traversal direction: callees, callers, or both (default)
    #[serde(default)]
    direction: Option<String>,
    /// Max traversal hops, 1-5 (default: 2)
    #[serde(default)]
    max_hops: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CuratedSuperpowersSetupParams {
    /// Include Aider setup instructions (default: true)
    #[serde(default = "default_true")]
    include_aider: bool,
    /// Include VS Code Copilot setup instructions (default: true)
    #[serde(default = "default_true")]
    include_vscode: bool,
}

fn default_true() -> bool {
    true
}

fn lock_conn(conn: &Arc<Mutex<Connection>>) -> Result<MutexGuard<'_, Connection>, rmcp::ErrorData> {
    conn.lock()
        .map_err(|_| rmcp::ErrorData::internal_error("database mutex poisoned", None))
}

/// Thin wrapper over the shared helper, mapping anyhow errors back to rmcp.
fn rank_wiki_entries(
    conn: &Connection,
    query: &str,
    limit_wiki: usize,
) -> Result<Vec<serde_json::Value>, rmcp::ErrorData> {
    shared_rank_wiki_entries(conn, query, limit_wiki).map_err(rmcp_internal)
}

/// Map ranked chunk rows to the sidecar's curated_search_code JSON shape.
fn code_rows_to_json(
    rows: Vec<curated_thoughts_tools::cli_common::RankedChunkRow>,
) -> Vec<serde_json::Value> {
    rows.into_iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "text": r.text,
                "doc_path": r.doc_path,
                "start_line": r.start_line,
                "end_line": r.end_line,
                "symbol": r.symbol_name,
                "strategy": r.language,
                "score": r.score
            })
        })
        .collect()
}

#[tool_router(server_handler)]
impl VaultMcpServer {
    /// Best-effort agent-access logging via the dedicated read-write connection.
    fn log_access(&self, tool: &str, entity_id: Option<&str>) {
        if let Ok(guard) = self.log_conn.lock() {
            tauri_app_lib::tool_dispatch::log_agent_access(&guard, "mcp-sidecar", tool, entity_id);
        }
    }
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
        self.log_access("vault_semantic_search", None);
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
        self.log_access("vault_related_chunks", None);
        serde_json::to_string(&hits)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }

    #[tool(
        name = "curated_recall_context",
        description = "Recall prioritized context from the Curated Thoughts wisdom layer (wiki) and vault code chunks for a coding task. Returns wiki entries first, then relevant code chunks, all ranked by relevance to the query."
    )]
    async fn curated_recall_context(
        &self,
        args: Parameters<CuratedRecallContextParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(CuratedRecallContextParams {
            query,
            limit_wiki,
            limit_code,
        }) = args;
        let limit_wiki = limit_wiki.unwrap_or(5);
        let limit_code = limit_code.unwrap_or(10);
        let conn = lock_conn(&self.conn)?;

        // Embed the query
        let query_embedding = embed_batch(&self.profile, vec![query.clone()])
            .map_err(|e| {
                rmcp::ErrorData::internal_error(format!("failed to embed query: {e}"), None)
            })?
            .into_iter()
            .next()
            .ok_or_else(|| {
                rmcp::ErrorData::internal_error("no embedding returned for query", None)
            })?;

        // Helper to fetch and rank chunks by similarity
        let fetch_ranked = |conn: &Connection, sql: &str, limit: usize| {
            fetch_ranked_chunks(conn, sql, &[], &query_embedding, limit)
                .map_err(rmcp_internal)
                .map(|rows| {
                    rows.into_iter()
                        .map(|r| {
                            serde_json::json!({
                                "id": r.id,
                                "text": r.text,
                                "doc_path": r.doc_path,
                                "start_line": r.start_line,
                                "end_line": r.end_line,
                                "symbol": r.symbol_name,
                                "score": r.score
                            })
                        })
                        .collect::<Vec<serde_json::Value>>()
                })
        };

        let wiki_entries = rank_wiki_entries(&conn, &query, limit_wiki)?;

        // Code chunks: real chunker strategies (ast_*). Vectors live in the
        // separate embeddings table.
        let code_sql = format!("{RECALL_CHUNKS_SQL_BASE}{RECALL_CHUNKS_AST_FILTER}");
        let code_chunks = fetch_ranked(&conn, &code_sql, limit_code)?;

        // Build response
        let response = serde_json::json!({
            "wiki_entries": wiki_entries,
            "code_chunks": code_chunks,
            "query": query
        });
        self.log_access("curated_recall_context", None);
        serde_json::to_string(&response)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }

    #[tool(
        name = "curated_get_wiki_entry",
        description = "Fetch full content of a specific Curated Thoughts wiki (wisdom layer) entry by topic or entity ID."
    )]
    async fn curated_get_wiki_entry(
        &self,
        args: Parameters<CuratedGetWikiEntryParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(CuratedGetWikiEntryParams { topic, entity_id }) = args;
        let conn = lock_conn(&self.conn)?;

        let (sql, params): (&str, Vec<Box<dyn rusqlite::ToSql>>) = if let Some(ref eid) = entity_id
        {
            (
                "SELECT body, 0 AS position, COALESCE(source_ref,''), NULL, NULL
                 FROM llm_wiki_entries
                 WHERE deleted_at IS NULL AND entity_id = ?1
                 ORDER BY updated_at",
                vec![Box::new(eid.clone())],
            )
        } else if let Some(ref topic) = topic {
            (
                "SELECT body, 0 AS position, COALESCE(source_ref,''), NULL, NULL
                 FROM llm_wiki_entries
                 WHERE deleted_at IS NULL
                   AND (title LIKE '%' || ?1 || '%'
                        OR body LIKE '%' || ?1 || '%'
                        OR tags LIKE '%' || ?1 || '%')
                 ORDER BY confidence DESC, updated_at",
                vec![Box::new(topic.clone())],
            )
        } else {
            return Err(rmcp::ErrorData::invalid_params(
                "must provide either topic or entity_id",
                None,
            ));
        };

        let mut stmt = conn.prepare(sql).map_err(|e| {
            rmcp::ErrorData::internal_error(format!("prepare wiki entry query: {e}"), None)
        })?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,      // text
                    row.get::<_, usize>(1)?,       // position
                    row.get::<_, String>(2)?,      // doc_path
                    row.get::<_, Option<u32>>(3)?, // start_line
                    row.get::<_, Option<u32>>(4)?, // end_line
                ))
            })
            .map_err(|e| {
                rmcp::ErrorData::internal_error(format!("execute wiki entry query: {e}"), None)
            })?;

        let mut full_text = String::new();
        let mut chunks = Vec::new();
        for row in rows {
            let (text, position, doc_path, start_line, end_line) = row.map_err(|e| {
                rmcp::ErrorData::internal_error(format!("read wiki entry row: {e}"), None)
            })?;
            full_text.push_str(&text);
            full_text.push('\n');
            chunks.push(serde_json::json!({
                "text": text,
                "position": position,
                "doc_path": doc_path,
                "start_line": start_line,
                "end_line": end_line
            }));
        }

        let response = serde_json::json!({
            "full_text": full_text.trim(),
            "chunks": chunks,
            "topic": topic,
            "entity_id": entity_id
        });
        self.log_access("curated_get_wiki_entry", entity_id.as_deref());
        serde_json::to_string(&response)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }

    #[tool(
        name = "curated_search_code",
        description = "Search Curated Thoughts code chunks (ast_* strategies) for a query or symbol, returning relevant code snippets for coding tasks."
    )]
    async fn curated_search_code(
        &self,
        args: Parameters<CuratedSearchCodeParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(CuratedSearchCodeParams {
            query,
            limit,
            symbol,
        }) = args;
        let limit = limit.unwrap_or(10);
        let conn = lock_conn(&self.conn)?;

        // Embed the query
        let query_embedding = embed_batch(&self.profile, vec![query.clone()])
            .map_err(|e| {
                rmcp::ErrorData::internal_error(format!("failed to embed query: {e}"), None)
            })?
            .into_iter()
            .next()
            .ok_or_else(|| {
                rmcp::ErrorData::internal_error("no embedding returned for query", None)
            })?;

        let mut sql = format!("{RECALL_CHUNKS_SQL_BASE}{RECALL_CHUNKS_AST_FILTER}");

        if let Some(ref sym) = symbol {
            sql.push_str(" AND c.symbol_name LIKE '%' || ?1 || '%'");
            // `params` stays empty here: the shared helper receives the raw
            // parameter slice below, so we pass the symbol directly.
            let rows = fetch_ranked_chunks(&conn, &sql, &[sym], &query_embedding, limit)
                .map_err(rmcp_internal)?;
            let results: Vec<serde_json::Value> = code_rows_to_json(rows);
            let response = serde_json::json!({
                "code_chunks": results,
                "query": query,
                "symbol_filter": symbol
            });
            self.log_access("curated_search_code", None);
            return serde_json::to_string(&response)
                .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None));
        }

        let rows = fetch_ranked_chunks(&conn, &sql, &[], &query_embedding, limit)
            .map_err(rmcp_internal)?;
        let results: Vec<serde_json::Value> = code_rows_to_json(rows);

        let response = serde_json::json!({
            "code_chunks": results,
            "query": query,
            "symbol_filter": symbol
        });
        self.log_access("curated_search_code", None);
        serde_json::to_string(&response)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }

    #[tool(
        name = "graph_neighbors",
        description = "Walk the code call/import graph from a root symbol using recursive traversal over curated_relationships (CALLS/IMPORTS edges). Returns neighbor chunks ranked by hop depth with doc path and symbol names. Resolve the root via curated_search_code first (chunk_id) or pass a symbol name directly."
    )]
    async fn graph_neighbors(
        &self,
        args: Parameters<GraphNeighborsParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(GraphNeighborsParams {
            chunk_id,
            symbol,
            direction,
            max_hops,
        }) = args;
        let conn = lock_conn(&self.conn)?;

        // Resolve root chunk id: explicit chunk_id wins, else match against
        // defined_symbol (definitions, lowercased+trimmed by the linker) with
        // symbol_name fallback. Definitions are the correct traversal root —
        // a ref chunk only has outgoing edges pointing AT the definition.
        let root_chunk_id: i64 = if let Some(id) = chunk_id {
            id
        } else if let Some(ref sym) = symbol {
            match resolve_symbol(&conn, sym).map_err(rmcp_internal)? {
                Some((id, _)) => id,
                None => {
                    return Err(rmcp::ErrorData::internal_error(
                        format!("no chunk found with symbol '{sym}'"),
                        None,
                    ))
                }
            }
        } else {
            return Err(rmcp::ErrorData::invalid_params(
                "provide either chunk_id or symbol",
                None,
            ));
        };

        // Root chunk's entity scope (matches how edges were written).
        let entity_id: String = conn
            .query_row(
                "SELECT entity_id FROM chunks WHERE id = ?1",
                rusqlite::params![root_chunk_id],
                |r| r.get(0),
            )
            .map_err(|e| {
                rmcp::ErrorData::internal_error(format!("root chunk not found: {e}"), None)
            })?;

        let hops = max_hops.unwrap_or(2).clamp(1, 5);
        let dir = direction.as_deref();
        if let Some(d) = dir {
            if !matches!(d, "callees" | "callers" | "both") {
                return Err(rmcp::ErrorData::invalid_params(
                    format!("invalid direction '{d}': expected callees, callers, or both"),
                    None,
                ));
            }
        }
        let neighbors =
            tauri_app_lib::graph::get_neighbors(&conn, root_chunk_id, &entity_id, hops, dir)
                .map_err(|e| {
                    rmcp::ErrorData::internal_error(format!("traversal failed: {e}"), None)
                })?;

        // Hard cap BEFORE enrichment: hub symbols at high depth can return
        // thousands of rows; don't run per-row lookups on all of them while
        // holding the DB lock.
        const MAX_NEIGHBORS: usize = 200;
        let total = neighbors.len();
        let capped: Vec<_> = neighbors.into_iter().take(MAX_NEIGHBORS).collect();

        // Enrich rows with doc path + symbol for agent consumption.
        let mut enriched = Vec::with_capacity(capped.len());
        {
            let mut stmt = conn
                .prepare(
                    "SELECT c.symbol_name, d.path FROM chunks c
                     JOIN documents d ON c.doc_id = d.id
                     WHERE c.id = ?1",
                )
                .map_err(|e| rmcp::ErrorData::internal_error(format!("prepare: {e}"), None))?;
            for n in &capped {
                let detail = stmt
                    .query_row(rusqlite::params![n.chunk_id], |r| {
                        Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?))
                    })
                    .unwrap_or_else(|_| (None, String::from("<deleted>")));
                enriched.push(serde_json::json!({
                    "chunk_id": n.chunk_id,
                    "depth": n.depth,
                    "rel_type": n.rel_type,
                    "symbol": detail.0,
                    "doc_path": detail.1,
                }));
            }
        }

        self.log_access("graph_neighbors", Some(&entity_id));
        serde_json::to_string(&serde_json::json!({
            "root_chunk_id": root_chunk_id,
            "direction": direction.as_deref().unwrap_or("both"),
            "max_hops": hops,
            "total_neighbors": total,
            "truncated": total > MAX_NEIGHBORS,
            "neighbors": enriched
        }))
        .map_err(|e| rmcp::ErrorData::internal_error(format!("json encode: {e}"), None))
    }

    #[tool(
        name = "curated_superpowers_setup",
        description = "Get step-by-step instructions to set up the Superpowers agentic skills framework for Aider and VS Code Copilot, integrated with Curated Thoughts MCP tools."
    )]
    async fn curated_superpowers_setup(
        &self,
        args: Parameters<CuratedSuperpowersSetupParams>,
    ) -> Result<String, rmcp::ErrorData> {
        let Parameters(CuratedSuperpowersSetupParams {
            include_aider,
            include_vscode,
        }) = args;
        let mut instructions = String::new();

        if include_aider {
            instructions.push_str("# Superpowers Setup for Aider\n");
            instructions.push_str(
                "Follow these steps to use Superpowers with Aider and Curated Thoughts:\n\n",
            );
            instructions.push_str("## Step 1: Install OpenSkills Globally\n");
            instructions.push_str("Open your system terminal (outside of Aider) and run:\n");
            instructions.push_str(
                "```bash\nbun add -g openskills\n# OR: npm install -g openskills\n```\n\n",
            );
            instructions.push_str("## Step 2: Fetch Superpowers Framework\n");
            instructions.push_str("Install Superpowers globally:\n");
            instructions.push_str(
                "```bash\nopenskills install obra/superpowers --universal --global\n```\n\n",
            );
            instructions.push_str("## Step 3: Sync Skills into Your Project\n");
            instructions.push_str("Navigate to your project directory and run:\n");
            instructions.push_str("```bash\ncd /path/to/your/project\nopenskills sync\n```\n\n");
            instructions.push_str("## Step 4: Configure Aider\n");
            instructions
                .push_str("Create or update `.aider.conf.yml` in your project root with:\n");
            instructions.push_str("```yaml\nmcp:\n  servers:\n    curated-thoughts:\n      command: \"curated-thoughts-mcp\"\n      args: []\nread:\n  - \".skills/superpowers/**/*\"\n  - \".skills/curated-thoughts/**/*\"\n```\n");
            instructions.push_str("Start Aider: `aider`. Aider will automatically load the Superpowers skills and Curated Thoughts MCP tools.\n\n");
            instructions.push_str("## Step 5: Execute Commands\n");
            instructions.push_str("Use natural language to trigger Superpowers workflows with Curated Thoughts context:\n");
            instructions.push_str("* “Run the superpowers brainstorming workflow on our next task, using Curated Thoughts to recall relevant wisdom.”\n");
            instructions.push_str("* “Execute a TDD test cycle for the new module using superpowers, and use `curated_recall_context` to fetch existing patterns.”\n\n");
        }

        if include_vscode {
            if !instructions.is_empty() {
                instructions.push_str("---\n\n");
            }
            instructions.push_str("# Superpowers Setup for VS Code Copilot\n");
            instructions.push_str("Follow these steps to use Superpowers with VS Code Copilot and Curated Thoughts:\n\n");
            instructions.push_str("## Step 1: Install OpenSkills Globally\n");
            instructions.push_str(
                "Same as Aider Step 1: `bun add -g openskills` or `npm install -g openskills`\n\n",
            );
            instructions.push_str("## Step 2: Fetch Superpowers Framework\n");
            instructions.push_str("Same as Aider Step 2: `openskills install obra/superpowers --universal --global`\n\n");
            instructions.push_str("## Step 3: Sync Skills into Your Project\n");
            instructions.push_str(
                "Same as Aider Step 3: `cd /path/to/your/project && openskills sync`\n\n",
            );
            instructions.push_str("## Step 4: Configure VS Code Copilot\n");
            instructions.push_str("Create `.vscode/mcp.json` in your project root with:\n");
            instructions.push_str("```json\n{\n  \"servers\": {\n    \"curated-thoughts\": {\n      \"type\": \"stdio\",\n      \"command\": \"curated-thoughts-mcp\",\n      \"args\": []\n    }\n  }\n}\n```\n");
            instructions.push_str("Restart VS Code. Copilot will discover the Curated Thoughts MCP server automatically.\n\n");
            instructions.push_str("## Step 5: Use Superpowers with Copilot\n");
            instructions.push_str("Open Copilot Chat and reference the Superpowers skills:\n");
            instructions.push_str("* “Use the Superpowers brainstorming workflow, and fetch relevant context from Curated Thoughts using `curated_recall_context`.”\n\n");
        }

        instructions.push_str("\n## Curated Thoughts MCP Tools Available\n");
        instructions.push_str("Once set up, the following tools are available via the `curated-thoughts` MCP server:\n");
        instructions.push_str("1. `vault_semantic_search`: Semantic search over vault chunks.\n");
        instructions
            .push_str("2. `vault_related_chunks`: List chunks related to a document path.\n");
        instructions.push_str(
            "3. `curated_recall_context`: Recall wisdom layer and code chunks for coding tasks.\n",
        );
        instructions.push_str("4. `curated_get_wiki_entry`: Fetch full wiki entry content.\n");
        instructions.push_str("5. `curated_search_code`: Search code chunks by query or symbol.\n");
        instructions.push_str("6. `curated_add_wisdom`: Add new entries to the wisdom layer.\n");
        instructions.push_str(
            "7. `curated_superpowers_setup`: Get this setup instructions (you just ran this!).\n",
        );

        let response = serde_json::json!({
            "instructions": instructions,
            "setup_complete": false,
            "next_step": "Follow the steps above to complete Superpowers setup for your preferred tool."
        });
        serde_json::to_string(&response)
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
    // Read-write connection used only for curated_agent_log inserts. Best-effort:
    // if the DB is genuinely read-only (or the file is missing), logging stays
    // silent but the server still runs.
    let log_conn = rusqlite::Connection::open_with_flags(
        &p.db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap_or_else(|e| {
        eprintln!("curated-thoughts-mcp: agent-log db unavailable ({e}); logging disabled");
        retrieval::open_brain_readonly(&p.db_path).expect("readonly fallback")
    });
    // Tolerate brief write contention from the desktop app / librarian instead of
    // dropping audit rows (best-effort: failures still never fail a tool call).
    let _ = log_conn.busy_timeout(std::time::Duration::from_millis(5000));

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
        log_conn: Arc::new(Mutex::new(log_conn)),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture table matching llm_wiki_entries' live shape but WITHOUT the
    /// NOT NULL constraint on updated_at, so we can exercise the defensive
    /// read path against mixed/legacy stored types (INTEGER, TEXT-form, NULL).
    fn open_fixture_db() -> Connection {
        let conn = tauri_app_lib::db::connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "DROP TABLE llm_wiki_entries;
             CREATE TABLE llm_wiki_entries (
               id TEXT PRIMARY KEY,
               entity_id TEXT NOT NULL,
               title TEXT NOT NULL,
               body TEXT NOT NULL,
               source_ref TEXT,
               confidence TEXT,
               updated_at,
               deleted_at INTEGER
             );",
        )
        .expect("recreate permissive fixture table");
        conn
    }

    fn insert_entry(
        conn: &Connection,
        id: &str,
        title: &str,
        updated_at_sql: &str,
        confidence: &str,
    ) {
        conn.execute(
            &format!(
                "INSERT INTO llm_wiki_entries
                     (id, entity_id, title, body, confidence, updated_at)
                 VALUES ('{id}', 'ent-{id}', '{title}', 'farmhouse annual report body',
                         '{confidence}', {updated_at_sql})"
            ),
            [],
        )
        .expect("insert fixture row");
    }

    #[test]
    fn recall_wiki_tolerates_mixed_updated_at_types() {
        let conn = open_fixture_db();
        insert_entry(&conn, "newest", "Farmhouse Notes", "1756000000", "inferred"); // INTEGER epoch
                                                                                    // TEXT-form value from an older writer: unparseable -> coerces to 0,
                                                                                    // but its higher confidence keeps the expected order deterministic.
        insert_entry(&conn, "middle", "Farmhouse Ledger", "'[PHONE]'", "verified");
        insert_entry(&conn, "unknown", "Farmhouse Chores", "NULL", "inferred"); // NULL

        let ranked = rank_wiki_entries(&conn, "farmhouse", 5).expect("ranking must not fail");
        assert_eq!(
            ranked.len(),
            3,
            "all rows must be returned despite mixed types"
        );
        let ids: Vec<&str> = ranked.iter().map(|v| v["id"].as_str().unwrap()).collect();
        // Ordered desc by NUMERIC updated_at (not lexicographic); ties broken
        // by confidence ('certain' first), NULL ranks as 0.
        assert_eq!(ids, vec!["middle", "newest", "unknown"]);
    }
}
