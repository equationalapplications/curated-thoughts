//! MCP stdio server for vault search (`rmcp`).

use std::sync::{Arc, Mutex, MutexGuard};

use rmcp::{handler::server::wrapper::Parameters, tool, tool_router, ServiceExt};
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::Deserialize;

use tauri_app_lib::embedder::{embed_batch, EmbedProfile};
use tauri_app_lib::retrieval;
use tauri_app_lib::search::{bytes_to_f32, cosine_similarity};

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
        let fetch_ranked_chunks = |conn: &Connection,
                                   sql: &str,
                                   params: &[&dyn rusqlite::ToSql],
                                   query_emb: &[f32],
                                   limit: usize|
         -> Result<Vec<serde_json::Value>, rmcp::ErrorData> {
            let mut stmt = conn.prepare(sql).map_err(|e| {
                rmcp::ErrorData::internal_error(format!("prepare chunk query: {e}"), None)
            })?;
            let rows = stmt
                .query_map(params, |row| {
                    Ok((
                        row.get::<_, i64>(0)?,            // id
                        row.get::<_, String>(1)?,         // text
                        row.get::<_, Vec<u8>>(2)?,        // embedding bytes
                        row.get::<_, String>(3)?,         // doc_path
                        row.get::<_, Option<u32>>(4)?,    // start_line
                        row.get::<_, Option<u32>>(5)?,    // end_line
                        row.get::<_, Option<String>>(6)?, // symbol (optional)
                    ))
                })
                .map_err(|e| {
                    rmcp::ErrorData::internal_error(format!("execute chunk query: {e}"), None)
                })?;

            let mut chunks_with_scores: Vec<(f32, serde_json::Value)> = Vec::new();
            for row in rows {
                let (id, text, emb_bytes, doc_path, start_line, end_line, symbol) =
                    row.map_err(|e| {
                        rmcp::ErrorData::internal_error(format!("read chunk row: {e}"), None)
                    })?;
                let chunk_emb = bytes_to_f32(&emb_bytes);
                if chunk_emb.len() != query_emb.len() {
                    continue; // skip chunks with mismatched embedding dimensions
                }
                let score = cosine_similarity(&query_emb, &chunk_emb);
                let chunk_json = serde_json::json!({
                    "id": id,
                    "text": text,
                    "doc_path": doc_path,
                    "start_line": start_line,
                    "end_line": end_line,
                    "symbol": symbol,
                    "score": score
                });
                chunks_with_scores.push((score, chunk_json));
            }

            // Sort descending by score, take top limit
            chunks_with_scores
                .sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            Ok(chunks_with_scores
                .into_iter()
                .take(limit)
                .map(|(_, v)| v)
                .collect())
        };

        // Fetch wiki entries (wisdom layer): tier = 'wiki'
        let wiki_sql = "
            SELECT c.id, c.text, c.embedding, d.path, c.start_line, c.end_line, NULL
            FROM chunks c
            JOIN documents d ON c.doc_id = d.id
            WHERE d.tier = 'wiki' AND c.embedding IS NOT NULL
        ";
        let wiki_entries = fetch_ranked_chunks(&conn, wiki_sql, &[], &query_embedding, limit_wiki)?;

        // Fetch code chunks: tier = 'user_doc', strategy = 'CodeLike'
        let code_sql = "
            SELECT c.id, c.text, c.embedding, d.path, c.start_line, c.end_line, c.symbol
            FROM chunks c
            JOIN documents d ON c.doc_id = d.id
            WHERE d.tier = 'user_doc' AND c.strategy = 'CodeLike' AND c.embedding IS NOT NULL
        ";
        let code_chunks = fetch_ranked_chunks(&conn, code_sql, &[], &query_embedding, limit_code)?;

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
                "SELECT c.text, c.position, d.path, c.start_line, c.end_line
                 FROM chunks c
                 JOIN documents d ON c.doc_id = d.id
                 WHERE d.tier = 'wiki' AND c.entity_id = ?1
                 ORDER BY c.position",
                vec![Box::new(eid.clone())],
            )
        } else if let Some(ref topic) = topic {
            (
                "SELECT c.text, c.position, d.path, c.start_line, c.end_line
                 FROM chunks c
                 JOIN documents d ON c.doc_id = d.id
                 WHERE d.tier = 'wiki' AND d.path LIKE '%' || ?1 || '%'
                 ORDER BY c.position",
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
        description = "Search Curated Thoughts code chunks (CodeLike strategy) for a query or symbol, returning relevant code snippets for coding tasks."
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

        let mut sql = "
            SELECT c.id, c.text, c.embedding, d.path, c.start_line, c.end_line, c.symbol, c.language
            FROM chunks c
            JOIN documents d ON c.doc_id = d.id
            WHERE d.tier = 'user_doc' AND c.strategy = 'CodeLike' AND c.embedding IS NOT NULL
        "
        .to_string();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(ref sym) = symbol {
            sql.push_str(" AND c.symbol LIKE '%' || ?1 || '%'");
            params.push(Box::new(sym.clone()));
        }

        let mut stmt = conn.prepare(&sql).map_err(|e| {
            rmcp::ErrorData::internal_error(format!("prepare code search query: {e}"), None)
        })?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                Ok((
                    row.get::<_, i64>(0)?,            // id
                    row.get::<_, String>(1)?,         // text
                    row.get::<_, Vec<u8>>(2)?,        // embedding bytes
                    row.get::<_, String>(3)?,         // doc_path
                    row.get::<_, Option<u32>>(4)?,    // start_line
                    row.get::<_, Option<u32>>(5)?,    // end_line
                    row.get::<_, Option<String>>(6)?, // symbol
                    row.get::<_, Option<String>>(7)?, // language
                ))
            })
            .map_err(|e| {
                rmcp::ErrorData::internal_error(format!("execute code search query: {e}"), None)
            })?;

        let mut chunks_with_scores: Vec<(f32, serde_json::Value)> = Vec::new();
        for row in rows {
            let (id, text, emb_bytes, doc_path, start_line, end_line, symbol, language) = row
                .map_err(|e| {
                    rmcp::ErrorData::internal_error(format!("read code chunk row: {e}"), None)
                })?;
            let chunk_emb = bytes_to_f32(&emb_bytes);
            if chunk_emb.len() != query_embedding.len() {
                continue;
            }
            let score = cosine_similarity(&query_embedding, &chunk_emb);
            let chunk_json = serde_json::json!({
                "id": id,
                "text": text,
                "doc_path": doc_path,
                "start_line": start_line,
                "end_line": end_line,
                "symbol": symbol,
                "language": language,
                "score": score
            });
            chunks_with_scores.push((score, chunk_json));
        }

        // Sort by score descending, take top limit
        chunks_with_scores
            .sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let results: Vec<serde_json::Value> = chunks_with_scores
            .into_iter()
            .take(limit)
            .map(|(_, v)| v)
            .collect();

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
