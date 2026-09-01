//! Transport-agnostic implementations of the five read-only vault/wiki tools shared by the
//! `--mcp` rmcp server and the Clanker cloud bridge. Both callers route through
//! `dispatch_tool_call` — one code path, two callers.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::embedder::EmbedProfile;
use crate::search::SearchResult;
use crate::wiki_graph::{
    self, TraverseDirection, WikiOntologyResult, WikiSearchHit, WikiTraverseResult,
    DEFAULT_MAX_DEPTH,
};

/// Typed error for unknown tool names so callers can classify without string matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownToolError(pub String);

impl std::fmt::Display for UnknownToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown tool: {}", self.0)
    }
}

impl std::error::Error for UnknownToolError {}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push("..");
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn safe_vault_relative_path(vault: &Path, user_path: &str) -> Option<PathBuf> {
    crate::vault::safe_vault_path(
        vault,
        user_path,
        crate::vault::READABLE_SUBDIRS,
        crate::vault::PathMode::MayCreate,
    )
    .ok()
}

/// Returns every path spelling worth trying against `documents.path` for a vault-relative or
/// absolute `doc_path`: raw input, vault-joined, canonicalized, and vault-relative forms.
pub fn build_path_candidates(doc_path: &str, vault_dir: Option<&Path>) -> Vec<String> {
    let p = Path::new(doc_path);
    let mut candidates: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if !candidates.iter().any(|e| e == &s) {
            candidates.push(s);
        }
    };

    if let Some(vault) = vault_dir {
        let vault_normalized = normalize_path_lexically(vault);

        if p.is_absolute() {
            let safe_validation_available = vault.canonicalize().is_ok();
            if safe_validation_available {
                if let Ok(rel) = crate::normalize_path_argument_to_vault_relative(doc_path, vault) {
                    if let Some(safe) = safe_vault_relative_path(vault, &rel) {
                        push(doc_path.to_string());
                        let abs_candidate = vault_normalized.join(&rel);
                        push(abs_candidate.to_string_lossy().into_owned());
                        if !rel.is_empty() {
                            push(rel.clone());
                        }
                        if let Ok(canon) = safe.canonicalize() {
                            push(canon.to_string_lossy().into_owned());
                        }
                        push(safe.to_string_lossy().into_owned());
                        return candidates;
                    }

                    push(doc_path.to_string());
                    let abs_candidate = vault_normalized.join(&rel);
                    push(abs_candidate.to_string_lossy().into_owned());
                    if !rel.is_empty() {
                        push(rel.clone());
                    }
                    return candidates;
                }
            }

            if !doc_path.as_bytes().contains(&0) {
                let accepted_candidate = p
                    .canonicalize()
                    .ok()
                    .filter(|canon| canon.starts_with(&vault_normalized))
                    .or_else(|| {
                        let normalized = normalize_path_lexically(p);
                        if normalized.starts_with(&vault_normalized) {
                            Some(normalized)
                        } else {
                            None
                        }
                    });

                if let Some(candidate) = accepted_candidate {
                    let mut candidate_string = candidate.to_string_lossy().into_owned();
                    if candidate_string != std::path::MAIN_SEPARATOR.to_string()
                        && candidate_string.ends_with(std::path::MAIN_SEPARATOR)
                    {
                        candidate_string = candidate_string
                            .trim_end_matches(std::path::MAIN_SEPARATOR)
                            .to_string();
                    }
                    push(doc_path.to_string());
                    push(candidate_string.clone());
                    if let Ok(rel) =
                        std::path::Path::new(&candidate_string).strip_prefix(&vault_normalized)
                    {
                        if !rel.as_os_str().is_empty() {
                            push(rel.to_string_lossy().into_owned());
                        }
                    }
                }
            }
        } else {
            let is_safe_relative = || {
                !doc_path.as_bytes().contains(&0)
                    && p.components().all(|c| {
                        !matches!(
                            c,
                            std::path::Component::ParentDir | std::path::Component::Prefix(_)
                        )
                    })
            };

            if is_safe_relative() {
                if let Some(safe) = safe_vault_relative_path(vault, doc_path) {
                    push(doc_path.to_string());
                    if let Ok(canon) = safe.canonicalize() {
                        if canon.starts_with(&vault_normalized) {
                            push(canon.to_string_lossy().into_owned());
                        }
                    }
                    push(safe.to_string_lossy().into_owned());
                    push(vault_normalized.join(p).to_string_lossy().into_owned());
                } else {
                    let joined = vault.join(p);
                    if let Ok(canon) = joined.canonicalize() {
                        if canon.starts_with(&vault_normalized) {
                            push(doc_path.to_string());
                            push(canon.to_string_lossy().into_owned());
                        }
                    } else {
                        let normalized = normalize_path_lexically(&joined);
                        if normalized.starts_with(&vault_normalized) {
                            push(doc_path.to_string());
                            push(normalized.to_string_lossy().into_owned());
                        }
                    }
                }
            }
        }
    } else {
        push(doc_path.to_string());
    }

    candidates
}

pub fn dispatch_vault_semantic_search(
    conn: &Connection,
    query_vec: &[f32],
    limit: Option<usize>,
) -> Result<Vec<SearchResult>> {
    let limit = limit.unwrap_or(10).clamp(1, 50);
    crate::search::semantic_search(conn, query_vec, limit)
}

pub fn dispatch_vault_related_chunks(
    conn: &Connection,
    candidates: &[String],
    limit: Option<usize>,
) -> Result<Vec<SearchResult>> {
    let limit = limit.unwrap_or(5).clamp(1, 10);
    crate::search::related_chunks_try_paths(conn, candidates, limit)
}

pub fn dispatch_wiki_search(
    conn: &Connection,
    query_vec: &[f32],
    entity_ids: Option<Vec<String>>,
    limit: Option<usize>,
) -> Result<Vec<WikiSearchHit>> {
    let limit = limit.unwrap_or(10).clamp(1, 25);
    // Pass the caller's intent through untouched. Substituting a default set
    // here is what made the default call path unable to match any row (#133).
    let refs: Option<Vec<&str>> = entity_ids
        .as_ref()
        .map(|ids| ids.iter().map(|s| s.as_str()).collect());
    wiki_graph::wiki_search(conn, query_vec, refs.as_deref(), limit)
}

pub fn dispatch_wiki_get_ontology(
    conn: &Connection,
    entity_id: &str,
) -> Result<WikiOntologyResult> {
    wiki_graph::wiki_get_ontology(conn, entity_id)
}

pub fn dispatch_wiki_traverse_graph(
    conn: &Connection,
    entity_id: &str,
    source_id: &str,
    max_depth: Option<usize>,
    direction: Option<String>,
    edge_types: Option<Vec<String>>,
) -> Result<WikiTraverseResult> {
    let max_depth = max_depth
        .unwrap_or(DEFAULT_MAX_DEPTH)
        .clamp(1, DEFAULT_MAX_DEPTH);
    let direction = TraverseDirection::parse(direction.as_deref().unwrap_or("both"));
    let edge_types = edge_types.unwrap_or_default();
    let edge_type_refs: Vec<&str> = edge_types.iter().map(|s| s.as_str()).collect();
    wiki_graph::wiki_traverse_graph(
        conn,
        entity_id,
        source_id,
        max_depth,
        direction,
        &edge_type_refs,
    )
}

pub fn dispatch_vault_write_note(
    vault_dir: &Path,
    path: &str,
    frontmatter: &crate::okf::OkfFrontmatter,
    body: &str,
) -> Result<crate::okf::WriteNoteResult> {
    // Thin adapter (spec v2): all logic lives in the `okf::write` core.
    // The MCP surface carries no separate If-Match parameter — the supplied
    // frontmatter's `updated_at` IS the If-Match token.
    crate::okf::write::write_note(
        vault_dir,
        path,
        frontmatter,
        body,
        frontmatter.updated_at.as_deref(),
    )
    .map_err(|e| anyhow::anyhow!("{}", e))
}

pub fn dispatch_vault_upsert_index_entry(
    vault_dir: &Path,
    index_path: &str,
    entry_name: &str,
    entry_path: &str,
    entry_type: &str,
    metadata: &Option<Value>,
) -> Result<crate::okf::UpsertResult> {
    crate::okf::write::upsert_index_entry(
        vault_dir,
        index_path,
        entry_name,
        entry_path,
        entry_type,
        metadata.as_ref(),
    )
    .map_err(|e| anyhow::anyhow!("{}", e))
}

#[derive(Clone)]
pub struct ToolDispatchContext {
    pub conn: Arc<Mutex<Connection>>,
    pub profile: EmbedProfile,
    pub vault_dir: Option<PathBuf>,
    /// Agent-log client label: "clanker-bridge" for cloud bridge, static label for local MCP (e.g. "local-mcp").
    /// The actual MCP client name is only known after the initialize handshake, which happens
    /// after this context is constructed; for now we use a static label.
    pub client: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct VaultSemanticSearchParams {
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct VaultRelatedChunksParams {
    #[serde(rename = "docPath", alias = "doc_path")]
    pub doc_path: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct WikiSearchParams {
    pub query: String,
    #[serde(default, rename = "entityIds", alias = "entity_ids")]
    pub entity_ids: Option<Vec<String>>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct WikiGetOntologyParams {
    #[serde(rename = "entityId", alias = "entity_id")]
    pub entity_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct WikiTraverseGraphParams {
    #[serde(rename = "entityId", alias = "entity_id")]
    pub entity_id: String,
    #[serde(rename = "sourceId", alias = "source_id")]
    pub source_id: String,
    #[serde(default, rename = "maxDepth", alias = "max_depth")]
    pub max_depth: Option<usize>,
    #[serde(default)]
    pub direction: Option<String>,
    #[serde(default, rename = "edgeTypes", alias = "edge_types")]
    pub edge_types: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct VaultWriteNoteParams {
    pub path: String,
    pub frontmatter: crate::okf::OkfFrontmatter,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct VaultUpsertIndexEntryParams {
    #[serde(rename = "indexPath", alias = "index_path")]
    pub index_path: String,
    #[serde(rename = "entryName", alias = "entry_name")]
    pub entry_name: String,
    #[serde(rename = "entryPath", alias = "entry_path")]
    pub entry_path: String,
    #[serde(rename = "entryType", alias = "entry_type")]
    pub entry_type: String,
    #[serde(default)]
    pub metadata: Option<Value>,
}

async fn embed_query(profile: &EmbedProfile, query: String) -> Result<Vec<f32>> {
    let profile = profile.clone();
    tokio::task::spawn_blocking(move || crate::embedder::embed_one(&profile, query)).await?
}

fn lock_conn(conn: &Arc<Mutex<Connection>>) -> Result<std::sync::MutexGuard<'_, Connection>> {
    conn.lock()
        .map_err(|_| anyhow::anyhow!("database mutex poisoned"))
}

/// Best-effort audit log for agent tool access. A failed log write must never fail
/// the tool call — tool availability wins over audit completeness.
pub fn log_agent_access(conn: &Connection, client: &str, tool: &str, entity_id: Option<&str>) {
    let _ = conn.execute(
        "INSERT INTO curated_agent_log (client, tool, operation, entity_id, summary)
         VALUES (?1, ?2, 'read', ?3, NULL)",
        rusqlite::params![client, tool, entity_id],
    );
}

/// Single entry point for all five read-only tools. Deserializes `params`, computes any
/// embedding *before* taking the DB lock (embedding is CPU/network bound; holding the mutex
/// during it would block concurrent callers), then dispatches to the matching pure
/// `dispatch_*` function on a blocking task. Both `mcp_server.rs`'s `#[tool]` methods and
/// `cloud_bridge::CloudBridgeClient` call this — it is the one code path behind two callers.
pub async fn dispatch_tool_call(
    ctx: &ToolDispatchContext,
    tool: &str,
    params: Value,
) -> Result<Value> {
    // Extract entity_id from params for agent logging (before deserialization consumes params)
    let entity_id = params
        .get("entity_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let tool_owned = tool.to_string();

    let client = ctx.client.clone();
    let conn_for_log = ctx.conn.clone();

    let result = match tool {
        "vault_semantic_search" => {
            let p: VaultSemanticSearchParams = serde_json::from_value(params)?;
            let query_vec = embed_query(&ctx.profile, p.query).await?;
            let conn = ctx.conn.clone();
            let limit = p.limit;
            let hits = tokio::task::spawn_blocking(move || {
                let conn_guard = lock_conn(&conn)?;
                dispatch_vault_semantic_search(&conn_guard, &query_vec, limit)
            })
            .await??;
            Ok(serde_json::to_value(hits)?)
        }
        "vault_related_chunks" => {
            let p: VaultRelatedChunksParams = serde_json::from_value(params)?;
            let candidates = build_path_candidates(&p.doc_path, ctx.vault_dir.as_deref());
            let conn = ctx.conn.clone();
            let limit = p.limit;
            let hits = tokio::task::spawn_blocking(move || {
                let conn_guard = lock_conn(&conn)?;
                dispatch_vault_related_chunks(&conn_guard, &candidates, limit)
            })
            .await??;
            Ok(serde_json::to_value(hits)?)
        }
        "wiki_search" => {
            let p: WikiSearchParams = serde_json::from_value(params)?;
            let query_vec = embed_query(&ctx.profile, p.query).await?;
            let conn = ctx.conn.clone();
            let (entity_ids, limit) = (p.entity_ids, p.limit);
            let hits = tokio::task::spawn_blocking(move || {
                let conn_guard = lock_conn(&conn)?;
                dispatch_wiki_search(&conn_guard, &query_vec, entity_ids, limit)
            })
            .await??;
            Ok(serde_json::to_value(hits)?)
        }
        "wiki_get_ontology" => {
            let p: WikiGetOntologyParams = serde_json::from_value(params)?;
            let conn = ctx.conn.clone();
            let result = tokio::task::spawn_blocking(move || {
                let conn_guard = lock_conn(&conn)?;
                dispatch_wiki_get_ontology(&conn_guard, &p.entity_id)
            })
            .await??;
            Ok(serde_json::to_value(result)?)
        }
        "wiki_traverse_graph" => {
            let p: WikiTraverseGraphParams = serde_json::from_value(params)?;
            let conn = ctx.conn.clone();
            let result = tokio::task::spawn_blocking(move || {
                let conn_guard = lock_conn(&conn)?;
                dispatch_wiki_traverse_graph(
                    &conn_guard,
                    &p.entity_id,
                    &p.source_id,
                    p.max_depth,
                    p.direction,
                    p.edge_types,
                )
            })
            .await??;
            Ok(serde_json::to_value(result)?)
        }
        "vault_write_note" => {
            let p: VaultWriteNoteParams = serde_json::from_value(params)?;
            let vault_dir = ctx
                .vault_dir
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("vault directory not configured"))?
                .clone();
            let result = tokio::task::spawn_blocking(move || {
                dispatch_vault_write_note(&vault_dir, &p.path, &p.frontmatter, &p.body)
            })
            .await??;
            Ok(serde_json::to_value(result)?)
        }
        "vault_upsert_index_entry" => {
            let p: VaultUpsertIndexEntryParams = serde_json::from_value(params)?;
            let vault_dir = ctx
                .vault_dir
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("vault directory not configured"))?
                .clone();
            let result = tokio::task::spawn_blocking(move || {
                dispatch_vault_upsert_index_entry(
                    &vault_dir,
                    &p.index_path,
                    &p.entry_name,
                    &p.entry_path,
                    &p.entry_type,
                    &p.metadata,
                )
            })
            .await??;
            Ok(serde_json::to_value(result)?)
        }
        other => Err(UnknownToolError(other.to_string()).into()),
    };

    // Agent access log: best-effort, never fail the tool call.
    // Log both successful and failed attempts (including unknown tool).
    let _ = tokio::task::spawn_blocking(move || {
        let guard = match conn_for_log.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        log_agent_access(&guard, &client, &tool_owned, entity_id.as_deref());
    })
    .await;

    result
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;

    fn seed_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE documents (id INTEGER PRIMARY KEY, path TEXT, status TEXT);
             CREATE TABLE chunks (id INTEGER PRIMARY KEY, doc_id INTEGER, chunk_text TEXT,
                position INTEGER, start_line INTEGER, end_line INTEGER, symbol_name TEXT,
                strategy TEXT, entity_id TEXT);
             CREATE TABLE embeddings (chunk_id INTEGER PRIMARY KEY, vector BLOB);
             INSERT INTO documents VALUES (1, 'notes/a.md', 'indexed');
             INSERT INTO chunks VALUES (1, 1, 'hello world', 0, 1, 2, NULL, 'plain', 'tier_working');
             INSERT INTO embeddings VALUES (1, x'0000803f00000000');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn vault_semantic_search_clamps_limit_and_defaults() {
        let conn = seed_db();
        let hits = dispatch_vault_semantic_search(&conn, &[1.0, 0.0], None).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].doc_path, "notes/a.md");
    }

    #[test]
    fn vault_semantic_search_clamps_limit_upper_bound() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE documents (id INTEGER PRIMARY KEY, path TEXT, status TEXT);
             CREATE TABLE chunks (id INTEGER PRIMARY KEY, doc_id INTEGER, chunk_text TEXT,
                position INTEGER, start_line INTEGER, end_line INTEGER, symbol_name TEXT,
                strategy TEXT, entity_id TEXT);
             CREATE TABLE embeddings (chunk_id INTEGER PRIMARY KEY, vector BLOB);",
        )
        .unwrap();
        let hits = dispatch_vault_semantic_search(&conn, &[1.0], Some(999)).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn wiki_search_with_no_entity_ids_searches_every_live_entry() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE llm_wiki_entries (id TEXT, entity_id TEXT, title TEXT,
                embedding_blob BLOB, deleted_at INTEGER);",
        )
        .unwrap();
        let blob = crate::wiki_graph::f32_vec_to_blob(&[1.0]);
        conn.execute(
            "INSERT INTO llm_wiki_entries VALUES ('e1', 'ent_448a', 'Entity One', ?1, NULL)",
            rusqlite::params![blob],
        )
        .unwrap();

        let hits = dispatch_wiki_search(&conn, &[1.0], None, None).unwrap();

        assert_eq!(hits.len(), 1, "the default path must reach ent_* rows");
        assert_eq!(hits[0].entity_id, "ent_448a");
    }

    #[test]
    fn wiki_traverse_graph_defaults_max_depth_and_direction() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE llm_wiki_entries (id TEXT, entity_id TEXT, title TEXT, deleted_at INTEGER);
             CREATE TABLE llm_wiki_edges (source_id TEXT, target_id TEXT, edge_type TEXT, entity_id TEXT);
             INSERT INTO llm_wiki_entries VALUES ('a', 'tier_fact', 'A', NULL);",
        )
        .unwrap();
        let result =
            dispatch_wiki_traverse_graph(&conn, "tier_fact", "a", None, None, None).unwrap();
        assert_eq!(result.nodes.len(), 1);
        assert!(!result.truncated);
    }
}

#[cfg(test)]
mod path_candidate_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn relative_path_no_vault_dir() {
        let candidates = build_path_candidates("notes/meeting.md", None);
        assert_eq!(candidates, vec!["notes/meeting.md".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn relative_path_with_vault_dir() {
        let vault = std::path::Path::new("/home/user/vault");
        let candidates = build_path_candidates("notes/meeting.md", Some(vault));
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0], "notes/meeting.md");
        assert_eq!(candidates[1], "/home/user/vault/notes/meeting.md");
    }

    #[cfg(unix)]
    #[test]
    fn absolute_path_under_vault_dir() {
        let vault = std::path::Path::new("/home/user/vault");
        let candidates = build_path_candidates("/home/user/vault/notes/meeting.md", Some(vault));
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0], "/home/user/vault/notes/meeting.md");
        assert_eq!(candidates[1], "notes/meeting.md");
    }

    #[cfg(unix)]
    #[test]
    fn absolute_path_under_vault_dir_with_canonical_prefix_alias() {
        let temp_dir = TempDir::new().unwrap();
        let vault_real = temp_dir.path().join("vault-real");
        std::fs::create_dir_all(&vault_real).unwrap();
        let vault_real = vault_real.canonicalize().unwrap();
        let vault_alias = temp_dir.path().join("vault-alias");
        std::os::unix::fs::symlink(&vault_real, &vault_alias).unwrap();

        let doc_path = vault_alias.join("notes/meeting.md");
        let expected_canonical = vault_real.join("notes/meeting.md");

        let candidates =
            build_path_candidates(doc_path.to_string_lossy().as_ref(), Some(&vault_real));

        assert!(candidates.contains(&doc_path.to_string_lossy().into_owned()));
        assert!(candidates.contains(&expected_canonical.to_string_lossy().into_owned()));
        assert!(candidates.contains(&"notes/meeting.md".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn absolute_path_outside_vault_dir_no_strip() {
        let vault = std::path::Path::new("/home/user/vault");
        let candidates = build_path_candidates("/tmp/other/file.md", Some(vault));
        assert!(
            candidates.is_empty(),
            "Outside-vault absolute paths should not be accepted"
        );
    }

    #[cfg(unix)]
    #[test]
    fn no_duplicates_when_path_matches_joined() {
        let vault = std::path::Path::new("/home/user/vault");
        let candidates = build_path_candidates("/home/user/vault/notes/meeting.md", Some(vault));
        let count = candidates
            .iter()
            .filter(|c| c.as_str() == "/home/user/vault/notes/meeting.md")
            .count();
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn absolute_path_with_parent_segments_outside_vault_is_rejected() {
        let vault = std::path::Path::new("/vault");
        let candidates = build_path_candidates("/vault/../outside.md", Some(vault));
        assert!(
            candidates.is_empty(),
            "Paths that normalize outside the vault must not be accepted"
        );
    }

    #[cfg(unix)]
    #[test]
    fn relative_path_with_parent_segments_outside_vault_is_rejected() {
        let vault = std::path::Path::new("/vault");
        let candidates = build_path_candidates("../outside.md", Some(vault));
        assert!(
            candidates.is_empty(),
            "Relative paths that resolve outside the vault must not be accepted"
        );
    }

    #[cfg(unix)]
    #[test]
    fn absolute_path_with_invalid_vault_path_is_rejected() {
        let vault = std::path::Path::new("/home/user/vault");
        let candidates = build_path_candidates("/home/user/vault/documents/evil\0.md", Some(vault));
        assert!(
            candidates.is_empty(),
            "Paths containing invalid characters must not be accepted"
        );
    }
}

#[cfg(test)]
mod dispatch_tool_call_tests {
    use super::*;

    fn seeded_ctx() -> ToolDispatchContext {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE llm_wiki_entries (id TEXT, entity_id TEXT, title TEXT, deleted_at INTEGER);
             CREATE TABLE llm_wiki_edges (source_id TEXT, target_id TEXT, edge_type TEXT, entity_id TEXT);
             CREATE TABLE llm_wiki_entity_manifests (
                entity_id TEXT PRIMARY KEY, mode TEXT NOT NULL, manifest_json TEXT NOT NULL, updated_at INTEGER);
             INSERT INTO llm_wiki_entries VALUES ('a', 'tier_fact', 'Entry A', NULL);",
        )
        .unwrap();
        ToolDispatchContext {
            conn: Arc::new(Mutex::new(conn)),
            profile: EmbedProfile::default(),
            vault_dir: None,
            client: "test-client".into(),
        }
    }

    #[tokio::test]
    async fn wiki_get_ontology_round_trips_through_json() {
        let ctx = seeded_ctx();
        let params = serde_json::json!({ "entityId": "tier_fact" });
        let result = dispatch_tool_call(&ctx, "wiki_get_ontology", params)
            .await
            .unwrap();
        assert_eq!(result["mode"], "off");
    }

    #[tokio::test]
    async fn wiki_traverse_graph_round_trips_defaults() {
        let ctx = seeded_ctx();
        let params = serde_json::json!({ "entityId": "tier_fact", "sourceId": "a" });
        let result = dispatch_tool_call(&ctx, "wiki_traverse_graph", params)
            .await
            .unwrap();
        assert_eq!(result["nodes"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn unknown_tool_name_errors() {
        let ctx = seeded_ctx();
        let err = dispatch_tool_call(&ctx, "delete_everything", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown tool"));
    }
}
