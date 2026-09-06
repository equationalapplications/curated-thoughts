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
    self, TraverseDirection, WikiContextResult, WikiOntologyResult, WikiSearchHit,
    WikiTraverseResult, DEFAULT_CONTEXT_DEPTH, DEFAULT_CONTEXT_MAX_FACTS, DEFAULT_MAX_DEPTH,
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
    tier: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<WikiSearchHit>> {
    if let Some(t) = tier.as_deref() {
        // Validate at the boundary so the caller gets a diagnostic rather than
        // an empty result set that looks like "no matches" (spec §3.1). The
        // vocabulary lives next to the V16 CHECK that enforces it, so the
        // boundary and the database can never drift apart.
        if !crate::db::schema::is_valid_tier(t) {
            anyhow::bail!(
                "tier must be one of {:?}, got {t:?}",
                crate::db::schema::VALID_TIERS
            );
        }
    }
    let limit = limit.unwrap_or(10).clamp(1, 25);
    // Pass the caller's intent through untouched. Substituting a default set
    // here is what made the default call path unable to match any row (#133).
    let refs: Option<Vec<&str>> = entity_ids
        .as_ref()
        .map(|ids| ids.iter().map(|s| s.as_str()).collect());
    wiki_graph::wiki_search(conn, query_vec, refs.as_deref(), tier.as_deref(), limit)
}

/// One-call retrieval: search plus the neighborhood around what was found.
///
/// Validates `tier` at the boundary exactly as `dispatch_wiki_search` does, so
/// the two tools cannot disagree about the vocabulary. `depth` is clamped, not
/// rejected — an out-of-range depth reports `truncated: true` instead of
/// failing the call (spec §4.1).
pub fn dispatch_wiki_context(
    conn: &Connection,
    query_vec: &[f32],
    tier: Option<String>,
    depth: Option<usize>,
    max_facts: Option<usize>,
) -> Result<WikiContextResult> {
    if let Some(t) = tier.as_deref() {
        if !crate::db::schema::is_valid_tier(t) {
            anyhow::bail!(
                "tier must be one of {:?}, got {t:?}",
                crate::db::schema::VALID_TIERS
            );
        }
    }
    wiki_graph::wiki_context(
        conn,
        query_vec,
        tier.as_deref(),
        depth.unwrap_or(DEFAULT_CONTEXT_DEPTH),
        max_facts.unwrap_or(DEFAULT_CONTEXT_MAX_FACTS),
    )
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

// ---------------------------------------------------------------------------
// Curated memory read helpers — ported from tools/src/queries.rs (SQL kept
// verbatim so sidecar behavior is unchanged). These back the curated_* MCP
// tools; the `tools` crate keeps its own copies and must NOT depend on this
// crate's internals.
// ---------------------------------------------------------------------------

/// One cosine-ranked code chunk from the chunks-JOIN-embeddings recall leg.
#[derive(Debug, Clone)]
pub(crate) struct RankedChunkRow {
    pub id: i64,
    pub text: String,
    pub doc_path: String,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub symbol_name: Option<String>,
    /// Second strategy-ish column (`c.strategy` in curated_search_code).
    #[allow(dead_code)] // ported verbatim from tools/src/queries.rs; the
    // curated_recall_context leg reads strategy only
    pub language: Option<String>,
    /// Kept for parity with the tools-crate struct; unused on this ported path.
    #[allow(dead_code)]
    pub entity_id: String,
    pub score: f32,
}

/// Recall-leg SQL over real chunker strategies (ast_*). Vectors live in the
/// separate embeddings table. With the AST predicate appended this matches the
/// sidecar's `curated_recall_context` / `curated_search_code` leg exactly.
pub(crate) const RECALL_CHUNKS_SQL_BASE: &str = "
    SELECT c.id, c.chunk_text, e.vector, d.path, c.start_line, c.end_line,
           c.symbol_name, c.strategy, c.entity_id
    FROM chunks c
    JOIN documents d ON c.doc_id = d.id
    JOIN embeddings e ON e.chunk_id = c.id
    WHERE d.status = 'indexed'
";
pub(crate) const RECALL_CHUNKS_AST_FILTER: &str = " AND c.strategy LIKE 'ast%'";

/// Little-endian f32 bytes -> Vec<f32> (mirrors `search::bytes_to_f32`).
fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

/// Cosine similarity in [0-ish, 1], clamped; 0.0 on length mismatch/zero norm
/// (mirrors `search::cosine_similarity`).
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
}

/// Coerce a stored `updated_at` cell into an integer epoch ranking key.
///
/// The live schema declares `updated_at INTEGER`, but desktop-app writes and
/// older sidecars may have produced TEXT-form values; NULL is also tolerated.
/// Any value that cannot be coerced ranks as 0 — acceptable for a sort key.
pub(crate) fn coerce_updated_at(value: Option<rusqlite::types::Value>) -> i64 {
    match value {
        Some(rusqlite::types::Value::Integer(i)) => i,
        Some(rusqlite::types::Value::Text(s)) => s.parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

/// Wisdom layer keyword ranking (ported verbatim from tools/src/queries.rs).
///
/// Term-overlap scoring over live (`deleted_at IS NULL`) `llm_wiki_entries`:
/// each matching term adds one overlap point; ties break by confidence string
/// (desc) then numeric `updated_at` (desc). Doubles as the keyword fallback
/// when no query embedding is available.
pub(crate) fn rank_wiki_entries(
    conn: &Connection,
    query: &str,
    limit_wiki: usize,
) -> Result<Vec<Value>> {
    // Keep terms of any length >= 2; short technical terms ("sql", "rag")
    // are often the most meaningful.
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_lowercase())
        .collect();
    if terms.is_empty() {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        "SELECT id, entity_id, title, body, source_ref, confidence,
                updated_at
         FROM llm_wiki_entries
         WHERE deleted_at IS NULL
           AND (title LIKE '%' || ?1 || '%'
                OR body LIKE '%' || ?1 || '%')",
    )?;
    // id -> (overlap_count, json_value, confidence_rank, updated_at)
    use std::collections::HashMap;
    let mut candidates: HashMap<String, (usize, Value, String, i64)> = HashMap::new();
    for term in &terms {
        let rows = stmt.query_map(rusqlite::params![term], |row| {
            Ok((
                row.get::<_, String>(0)?,                         // id (TEXT PK)
                row.get::<_, Option<String>>(1)?,                 // entity_id
                row.get::<_, Option<String>>(2)?,                 // title
                row.get::<_, Option<String>>(3)?,                 // body
                row.get::<_, Option<String>>(4)?,                 // source_ref
                row.get::<_, Option<String>>(5)?,                 // confidence (TEXT)
                row.get::<_, Option<rusqlite::types::Value>>(6)?, // updated_at
            ))
        })?;
        for row in rows {
            // A single malformed row must never abort the whole tool call:
            // log it and skip.
            let (id, entity_id, title, text, source_ref, confidence, updated_at_raw) = match row {
                Ok(row) => row,
                Err(e) => {
                    eprintln!("curated-thoughts-mcp: skipping unreadable wiki row: {e}");
                    continue;
                }
            };
            let updated_at = coerce_updated_at(updated_at_raw);
            let entry = candidates.entry(id.clone()).or_insert_with(|| {
                let v = serde_json::json!({
                    "id": id,
                    "entity_id": entity_id,
                    "title": title,
                    "text": text,
                    "source_ref": source_ref,
                    "confidence": confidence,
                });
                // Higher confidence string sorts later; rank key is
                // inverted for descending sort convenience.
                (0, v, confidence.clone().unwrap_or_default(), updated_at)
            });
            entry.0 += 1; // one overlap point per matching term
        }
    }
    let mut ranked: Vec<_> = candidates.into_iter().collect();
    ranked.sort_by(|a, b| {
        b.1 .0
            .cmp(&a.1 .0) // term overlap desc
            .then_with(|| b.1 .2.cmp(&a.1 .2)) // confidence desc
            .then_with(|| b.1 .3.cmp(&a.1 .3)) // updated_at desc (numeric)
    });
    Ok(ranked
        .into_iter()
        .take(limit_wiki)
        .map(|(_, (_, v, _, _))| v)
        .collect())
}

/// Fetch and rank chunk rows by cosine similarity against `query_emb`.
/// Chunks with mismatched embedding dimensions are skipped; results are
/// sorted by score descending and truncated to `limit`. (Ported from
/// tools/src/queries.rs; errors adapted to anyhow.)
pub(crate) fn fetch_ranked_chunks(
    conn: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
    query_emb: &[f32],
    limit: usize,
) -> Result<Vec<RankedChunkRow>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params, |row| {
        Ok((
            row.get::<_, i64>(0)?,            // id
            row.get::<_, String>(1)?,         // text
            row.get::<_, Vec<u8>>(2)?,        // embedding bytes
            row.get::<_, String>(3)?,         // doc_path
            row.get::<_, Option<u32>>(4)?,    // start_line
            row.get::<_, Option<u32>>(5)?,    // end_line
            row.get::<_, Option<String>>(6)?, // symbol (optional)
            row.get::<_, Option<String>>(7)?, // strategy/language (optional)
            row.get::<_, Option<String>>(8)?, // entity_id (optional)
        ))
    })?;

    let mut scored: Vec<RankedChunkRow> = Vec::new();
    for row in rows {
        let (id, text, emb_bytes, doc_path, start_line, end_line, symbol, language, entity_id) =
            row?;
        let chunk_emb = bytes_to_f32(&emb_bytes);
        if chunk_emb.len() != query_emb.len() {
            continue; // skip chunks with mismatched embedding dimensions
        }
        let score = cosine_similarity(query_emb, &chunk_emb);
        scored.push(RankedChunkRow {
            id,
            text,
            doc_path,
            start_line,
            end_line,
            symbol_name: symbol,
            language,
            entity_id: entity_id.unwrap_or_default(),
            score,
        });
    }

    // Sort descending by score, take top limit
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit);
    Ok(scored)
}

/// Chunk-row -> JSON mapper (ported from the coding server bin's
/// `code_rows_to_json`).
pub(crate) fn code_rows_to_json(rows: Vec<RankedChunkRow>) -> Vec<Value> {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct CuratedRecallContextParams {
    /// Coding task query to recall context for
    pub query: String,
    /// Max number of wisdom layer (wiki) entries to return (default: 5)
    #[serde(default)]
    pub limit_wiki: Option<usize>,
    /// Max number of code chunks to return (default: 10)
    #[serde(default)]
    pub limit_code: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct CuratedGetWikiEntryParams {
    /// Topic to search for in wiki entries (title/body/tags match)
    #[serde(default)]
    pub topic: Option<String>,
    /// Specific entity ID of the wiki entry to fetch
    #[serde(default)]
    pub entity_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct CuratedSearchCodeParams {
    /// Query to search code chunks
    pub query: String,
    /// Max number of code chunks to return (default: 10)
    #[serde(default)]
    pub limit: Option<usize>,
    /// Optional symbol name to filter code chunks (e.g., function name)
    #[serde(default)]
    pub symbol: Option<String>,
}

// ---------------------------------------------------------------------------
// Curated memory read dispatchers (ported from the coding server sidecar's
// handler bodies; rmcp::ErrorData -> anyhow).
// ---------------------------------------------------------------------------

/// Recall prioritized context for a coding task: keyword-ranked wisdom
/// entries plus AST-strategy code chunks ranked by cosine similarity.
pub async fn dispatch_curated_recall_context(
    ctx: &ToolDispatchContext,
    p: CuratedRecallContextParams,
) -> Result<Value> {
    let limit_wiki = p.limit_wiki.unwrap_or(5);
    let limit_code = p.limit_code.unwrap_or(10);

    // Embed OUTSIDE the DB lock (blocking network call).
    let query_vec = embed_query(&ctx.profile, p.query.clone()).await?;

    let conn = ctx.conn.clone();
    let query = p.query.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<Value> {
        let conn_guard = lock_conn(&conn)?;
        let wiki_entries = rank_wiki_entries(&conn_guard, &query, limit_wiki)?;

        // Code chunks: real chunker strategies (ast_*). Vectors live in the
        // separate embeddings table.
        let code_sql = format!("{RECALL_CHUNKS_SQL_BASE}{RECALL_CHUNKS_AST_FILTER}");
        let code_chunks = code_rows_to_json(fetch_ranked_chunks(
            &conn_guard,
            &code_sql,
            &[],
            &query_vec,
            limit_code,
        )?);

        Ok(serde_json::json!({
            "wiki_entries": wiki_entries,
            "code_chunks": code_chunks,
            "query": query
        }))
    })
    .await
    .map_err(|e| anyhow::anyhow!("recall task join error: {e}"))??;
    Ok(result)
}

/// Fetch full content of wiki (wisdom layer) entries by entity_id or topic.
/// Precedence (spec §6): when both are supplied, `entity_id` wins and
/// `topic` is ignored.
pub async fn dispatch_curated_get_wiki_entry(
    ctx: &ToolDispatchContext,
    p: CuratedGetWikiEntryParams,
) -> Result<Value> {
    let conn = ctx.conn.clone();
    let topic = p.topic.clone();
    let entity_id = p.entity_id.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<Value> {
        let conn_guard = lock_conn(&conn)?;

        let (sql, params): (&str, Vec<Box<dyn rusqlite::ToSql>>) = if let Some(ref eid) = entity_id
        {
            // The plan's precedence fixture passes the wiki ENTRY id ("w1")
            // as `entity_id`, so accept either the entity grouping id or the
            // per-entry id — entry ids are unique keys, so this only ever
            // widens the group with at most that one entry.
            (
                "SELECT body, 0 AS position, COALESCE(source_ref,''), NULL, NULL
                 FROM llm_wiki_entries
                 WHERE deleted_at IS NULL AND (entity_id = ?1 OR id = ?1)
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
            anyhow::bail!("must provide either topic or entity_id");
        };

        let mut stmt = conn_guard
            .prepare(sql)
            .map_err(|e| anyhow::anyhow!("prepare wiki entry query: {e}"))?;
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
            .map_err(|e| anyhow::anyhow!("execute wiki entry query: {e}"))?;

        let mut full_text = String::new();
        let mut chunks = Vec::new();
        for row in rows {
            let (text, position, doc_path, start_line, end_line) =
                row.map_err(|e| anyhow::anyhow!("read wiki entry row: {e}"))?;
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

        Ok(serde_json::json!({
            "full_text": full_text.trim(),
            "chunks": chunks,
            "topic": topic,
            "entity_id": entity_id
        }))
    })
    .await
    .map_err(|e| anyhow::anyhow!("get_entry task join error: {e}"))??;
    Ok(result)
}

/// Search code chunks (ast_* strategies) by query embedding, optionally
/// narrowed to a symbol name.
pub async fn dispatch_curated_search_code(
    ctx: &ToolDispatchContext,
    p: CuratedSearchCodeParams,
) -> Result<Value> {
    let limit = p.limit.unwrap_or(10);

    // Embed OUTSIDE the DB lock (blocking network call).
    let query_vec = embed_query(&ctx.profile, p.query.clone()).await?;

    let conn = ctx.conn.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<Value> {
        let conn_guard = lock_conn(&conn)?;
        let sql = format!("{RECALL_CHUNKS_SQL_BASE}{RECALL_CHUNKS_AST_FILTER}");

        if let Some(ref sym) = p.symbol {
            let with_symbol = format!("{sql} AND c.symbol_name LIKE '%' || ?1 || '%'");
            // The shared helper takes the symbol as its sole parameter.
            let rows = fetch_ranked_chunks(&conn_guard, &with_symbol, &[sym], &query_vec, limit)?;
            return Ok(serde_json::json!({
                "code_chunks": code_rows_to_json(rows),
                "query": p.query,
                "symbol_filter": p.symbol
            }));
        }

        let rows = fetch_ranked_chunks(&conn_guard, &sql, &[], &query_vec, limit)?;
        Ok(serde_json::json!({
            "code_chunks": code_rows_to_json(rows),
            "query": p.query,
            "symbol_filter": p.symbol
        }))
    })
    .await
    .map_err(|e| anyhow::anyhow!("search_code task join error: {e}"))??;
    Ok(result)
}

impl ToolDispatchContext {
    /// Run `f` with the lazily-opened read-write brain connection.
    ///
    /// First call opens `db_path` with `SQLITE_OPEN_READ_WRITE` (NO create —
    /// a missing brain file is an error, never a fresh DB) plus a 5s busy
    /// timeout, and caches the connection for subsequent calls. The async
    /// surface hides the blocking open/lock behind `spawn_blocking`; callers
    /// `await` directly (there is no `run_sync` helper).
    pub async fn with_rw<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        // Get-or-open the cached RW connection.
        let existing = {
            let cache = self
                .rw_conn
                .lock()
                .map_err(|_| anyhow::anyhow!("rw_conn mutex poisoned"))?;
            cache.clone()
        };

        let conn: Arc<Mutex<Connection>> = match existing {
            Some(c) => c,
            None => {
                let db_path = self.db_path.clone();
                let opened = tokio::task::spawn_blocking(move || {
                    Connection::open_with_flags(
                        &db_path,
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
                    )
                    .map_err(|e| {
                        anyhow::anyhow!("read-write brain open failed ({}): {e}", db_path.display())
                    })
                    .map(|conn| {
                        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
                        Arc::new(Mutex::new(conn))
                    })
                })
                .await
                .map_err(|e| anyhow::anyhow!("rw open task join error: {e}"))??;

                // Cache only on success; a concurrent caller may have won the
                // race — either handle works, keep the first one stored.
                let mut cache = self
                    .rw_conn
                    .lock()
                    .map_err(|_| anyhow::anyhow!("rw_conn mutex poisoned"))?;
                if let Some(c) = cache.as_ref() {
                    c.clone()
                } else {
                    *cache = Some(opened.clone());
                    opened
                }
            }
        };

        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| anyhow::anyhow!("rw connection mutex poisoned"))?;
            f(&mut guard)
        })
        .await
        .map_err(|e| anyhow::anyhow!("rw task join error: {e}"))?
    }

    /// Compute the embedding blob for a wisdom body OUTSIDE any DB lock
    /// (blocking network call). Provider failures collapse to `None`; the
    /// embedding sweep fills the blob later.
    pub fn precompute_wisdom_embedding(&self, body: &str) -> Option<Vec<u8>> {
        crate::db::wisdom::precompute_entry_embedding(Some(&self.profile), body)
    }
}

// ---------------------------------------------------------------------------
// Curated memory write dispatchers + fail-closed audit logging (spec §5/§7).
// Every DB write goes through db::wisdom; audit INSERT failures FAIL the
// tool call (fail-closed) — the existing 8 non-curated tools keep their
// best-effort path (log_agent_access) untouched.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct CuratedAddWisdomParams {
    /// Entity to attach the new wisdom entry to (must be active)
    pub entity_id: String,
    /// Body of the wisdom entry (title is derived from it)
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct CuratedUpdateWisdomParams {
    /// Owning entity of the wisdom entry
    pub entity_id: String,
    /// Id of the wisdom entry to rewrite
    pub wisdom_id: String,
    /// New body (title re-derived from it)
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct CuratedArchiveWisdomParams {
    /// Owning entity of the wisdom entry
    pub entity_id: String,
    /// Id of the wisdom entry to soft-delete
    pub wisdom_id: String,
}

/// Fail-closed audit log for curated tool calls (spec §7): a failed INSERT
/// propagates and fails the tool call, unlike the best-effort
/// [`log_agent_access`] used by the eight pre-existing read tools
/// (their migration to fail-closed is tracked separately).
pub fn log_agent_access_checked(
    conn: &Connection,
    client: &str,
    tool: &str,
    entity_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO curated_agent_log (client, tool, operation, entity_id, summary)
         VALUES (?1, ?2, 'write', ?3, NULL)",
        rusqlite::params![client, tool, entity_id],
    )
    .map_err(|e| anyhow::anyhow!("audit log insert failed for {tool}: {e}"))?;
    Ok(())
}

/// Reload one live wisdom entry as JSON (per-entry query shape shared with
/// the coding server). Used by `dispatch_curated_update_wisdom` so the
/// response is read back from the DB, never echoed from the request.
fn load_wisdom_json(conn: &Connection, entity_id: &str, wisdom_id: &str) -> Result<Value> {
    let (id, ent, title, body, source_type): (String, String, String, String, String) = conn
        .query_row(
            "SELECT id, entity_id, title, body, source_type
             FROM llm_wiki_entries
             WHERE entity_id = ?1 AND id = ?2 AND deleted_at IS NULL",
            rusqlite::params![entity_id, wisdom_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                ))
            },
        )
        .map_err(|e| {
            anyhow::anyhow!("reloading updated wisdom {wisdom_id} under {entity_id}: {e}")
        })?;
    Ok(serde_json::json!({
        "id": id,
        "entity_id": ent,
        "title": title,
        "body": body,
        "source_type": source_type,
    }))
}

/// Add a user-stated wisdom entry through the `db::wisdom` core.
///
/// Embedding is precomputed OUTSIDE the RW lock; the audit row is written in
/// the SAME `with_rw` closure, after the mutation and BEFORE returning — an
/// audit failure aborts the response even though the mutation itself already
/// committed (the outbox/mutation is not rolled back).
pub async fn dispatch_curated_add_wisdom(
    ctx: &ToolDispatchContext,
    p: CuratedAddWisdomParams,
) -> Result<Value> {
    let blob = ctx.precompute_wisdom_embedding(&p.body);
    let client = ctx.client.clone();
    let entity_id = p.entity_id.clone();
    let wisdom = ctx
        .with_rw(move |conn| {
            let wisdom =
                crate::db::wisdom::add_wisdom_with_blob(conn, &p.entity_id, &p.body, blob)?;
            // Audit follows the mutation; the mutation itself is not rolled back.
            log_agent_access_checked(conn, &client, "curated_add_wisdom", Some(&p.entity_id))?;
            Ok(wisdom)
        })
        .await?;
    Ok(serde_json::json!({
        "id": wisdom.id,
        "entity_id": entity_id,
        "title": wisdom.title,
        "body": wisdom.body,
        "source_type": wisdom.source_type,
    }))
}

/// Rewrite a wisdom entry's body through the `db::wisdom` core and return the
/// RELOADED entry (read back from the DB; `update_wisdom_with_blob` returns
/// `Result<()>`, so echoing the request would fabricate the response).
/// Audit contract matches [`dispatch_curated_add_wisdom`].
pub async fn dispatch_curated_update_wisdom(
    ctx: &ToolDispatchContext,
    p: CuratedUpdateWisdomParams,
) -> Result<Value> {
    let blob = ctx.precompute_wisdom_embedding(&p.body);
    let client = ctx.client.clone();
    ctx.with_rw(move |conn| {
        crate::db::wisdom::update_wisdom_with_blob(
            conn,
            &p.entity_id,
            &p.wisdom_id,
            &p.body,
            blob,
        )?;
        let reloaded = load_wisdom_json(conn, &p.entity_id, &p.wisdom_id)?;
        // Audit follows the mutation; the mutation itself is not rolled back.
        log_agent_access_checked(
            conn,
            &client,
            "curated_update_wisdom",
            Some(&p.entity_id),
        )?;
        Ok(reloaded)
    })
    .await
}

/// Soft-delete a wisdom entry through the `db::wisdom` core.
/// Audit contract matches [`dispatch_curated_add_wisdom`].
pub async fn dispatch_curated_archive_wisdom(
    ctx: &ToolDispatchContext,
    p: CuratedArchiveWisdomParams,
) -> Result<Value> {
    let client = ctx.client.clone();
    ctx.with_rw(move |conn| {
        crate::db::wisdom::archive_wisdom(conn, &p.entity_id, &p.wisdom_id)?;
        // Audit follows the mutation; the mutation itself is not rolled back.
        log_agent_access_checked(
            conn,
            &client,
            "curated_archive_wisdom",
            Some(&p.entity_id),
        )?;
        Ok(serde_json::json!({
            "archived": true,
            "wisdom_id": p.wisdom_id,
        }))
    })
    .await
}

#[derive(Clone)]
pub struct ToolDispatchContext {
    pub conn: Arc<Mutex<Connection>>,
    pub profile: EmbedProfile,
    pub vault_dir: Option<PathBuf>,
    /// Path to the brain DB file, used to lazily open the RW connection for
    /// curated write tools. Opens must NEVER create the file.
    pub db_path: PathBuf,
    /// Lazily-opened read-write brain connection (curated write tools +
    /// fail-closed audit logging). `None` until the first `with_rw` call.
    pub rw_conn: Arc<Mutex<Option<Arc<Mutex<Connection>>>>>,
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
    /// Optional stored-tier filter: "fact" or "wisdom". Omit for every entry.
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp-server", derive(schemars::JsonSchema))]
pub struct WikiContextParams {
    pub query: String,
    /// Traversal depth around each fact. Clamped to 1..=3.
    #[serde(default)]
    pub depth: Option<usize>,
    /// How many scored facts to seed the walk with.
    #[serde(default, rename = "maxFacts", alias = "max_facts")]
    pub max_facts: Option<usize>,
    /// Optional stored-tier filter: "fact" or "wisdom". Omit for every entry.
    #[serde(default)]
    pub tier: Option<String>,
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
            let (entity_ids, tier, limit) = (p.entity_ids, p.tier, p.limit);
            let hits = tokio::task::spawn_blocking(move || {
                let conn_guard = lock_conn(&conn)?;
                dispatch_wiki_search(&conn_guard, &query_vec, entity_ids, tier, limit)
            })
            .await??;
            Ok(serde_json::to_value(hits)?)
        }
        "wiki_context" => {
            let p: WikiContextParams = serde_json::from_value(params)?;
            let query_vec = embed_query(&ctx.profile, p.query).await?;
            let conn = ctx.conn.clone();
            let (tier, depth, max_facts) = (p.tier, p.depth, p.max_facts);
            let result = tokio::task::spawn_blocking(move || {
                let conn_guard = lock_conn(&conn)?;
                dispatch_wiki_context(&conn_guard, &query_vec, tier, depth, max_facts)
            })
            .await??;
            Ok(serde_json::to_value(result)?)
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

    /// In-memory connection with the schema the tier-filter tests need.
    /// Mirrors the inline pattern of `wiki_search_with_no_entity_ids_searches_every_live_entry`
    /// but adds the `tier` column introduced by V16.
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE llm_wiki_entries (id TEXT, entity_id TEXT, title TEXT,
                tier TEXT NULL, embedding_blob BLOB, deleted_at INTEGER);",
        )
        .unwrap();
        conn
    }

    /// Builds three live embedded entries: one 'fact', one 'wisdom', one NULL.
    fn seed_tiered_entries(conn: &Connection) {
        let blob = 1.0f32.to_le_bytes().to_vec();
        for (id, tier) in [("f1", Some("fact")), ("w1", Some("wisdom")), ("n1", None)] {
            conn.execute(
                "INSERT INTO llm_wiki_entries (id, entity_id, title, tier, embedding_blob)
                 VALUES (?1, 'ent_1', ?1, ?2, ?3)",
                rusqlite::params![id, tier, blob],
            )
            .unwrap();
        }
    }

    #[test]
    fn wiki_search_returns_tier_on_every_hit() {
        let conn = test_conn();
        seed_tiered_entries(&conn);
        let hits = dispatch_wiki_search(&conn, &[1.0], None, None, None).unwrap();
        let mut got: Vec<_> = hits
            .iter()
            .map(|h| (h.id.as_str(), h.tier.as_deref()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![("f1", Some("fact")), ("n1", None), ("w1", Some("wisdom"))]
        );
    }

    #[test]
    fn wiki_search_tier_filter_returns_only_matching_entries() {
        let conn = test_conn();
        seed_tiered_entries(&conn);
        let hits = dispatch_wiki_search(&conn, &[1.0], None, Some("wisdom".into()), None).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "w1");
    }

    #[test]
    fn wiki_search_without_tier_filter_still_returns_every_live_entry() {
        // The #133 contract: omitting the filter must not narrow anything.
        let conn = test_conn();
        seed_tiered_entries(&conn);
        let hits = dispatch_wiki_search(&conn, &[1.0], None, None, None).unwrap();
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn wiki_context_rejects_an_out_of_vocabulary_tier() {
        // Same vocabulary as wiki_search — both boundaries read the set that
        // the V16 CHECK enforces, so they cannot drift apart.
        let conn = test_conn();
        let err = dispatch_wiki_context(&conn, &[1.0], Some("anchor".into()), None, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("anchor"),
            "the diagnostic names the bad value: {err}"
        );
    }

    #[test]
    fn wiki_search_tier_filter_is_independent_of_entity_ids() {
        // Tier and partition are orthogonal — neither substitutes for the other.
        let conn = test_conn();
        seed_tiered_entries(&conn);
        let hits = dispatch_wiki_search(
            &conn,
            &[1.0],
            Some(vec!["ent_1".into()]),
            Some("fact".into()),
            None,
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "f1");

        let none = dispatch_wiki_search(
            &conn,
            &[1.0],
            Some(vec!["ent_absent".into()]),
            Some("fact".into()),
            None,
        )
        .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn wiki_search_with_no_entity_ids_searches_every_live_entry() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE llm_wiki_entries (id TEXT, entity_id TEXT, title TEXT,
                tier TEXT NULL, embedding_blob BLOB, deleted_at INTEGER);",
        )
        .unwrap();
        let blob = crate::wiki_graph::f32_vec_to_blob(&[1.0]);
        conn.execute(
            "INSERT INTO llm_wiki_entries VALUES ('e1', 'ent_448a', 'Entity One', NULL, ?1, NULL)",
            rusqlite::params![blob],
        )
        .unwrap();

        let hits = dispatch_wiki_search(&conn, &[1.0], None, None, None).unwrap();

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
            db_path: PathBuf::from("/nonexistent/brain.db"),
            rw_conn: Arc::new(Mutex::new(None)),
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

    /// `wiki_context` must be reachable through the shared dispatcher — the
    /// tool shipped in the spec but not in the `match`, so every call returned
    /// "unknown tool".
    #[tokio::test]
    async fn wiki_context_is_a_known_tool() {
        let ctx = seeded_ctx();
        let err = dispatch_tool_call(&ctx, "wiki_context", serde_json::json!({ "query": "x" }))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            !err.contains("unknown tool"),
            "wiki_context must be registered; got: {err}"
        );
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

#[cfg(test)]
mod curated_memory_tests {
    use super::*;
    use rusqlite::Connection;

    /// File-backed brain fixture (spec §9): tests exercising BOTH the RO and RW
    /// connections must share one real DB file — bare `open_in_memory()` is
    /// per-connection-private. Table shapes mirror the live DDL in
    /// `db/schema.rs` (documents/chunks/embeddings) and `db/okf_ddl.rs`
    /// (llm_wiki_entries/curated_entities/curated_agent_log/llm_wiki_outbox).
    pub(crate) fn seed_file_db(dir: &std::path::Path) -> Connection {
        let conn = Connection::open(dir.join("brain.db")).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE llm_wiki_entries (
                id TEXT PRIMARY KEY,
                entity_id TEXT NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                tags TEXT NOT NULL DEFAULT '[]',
                confidence TEXT NOT NULL DEFAULT 'inferred',
                source_type TEXT NOT NULL DEFAULT 'user_stated',
                source_hash TEXT,
                source_ref TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                last_accessed_at INTEGER,
                access_count INTEGER NOT NULL DEFAULT 0,
                deleted_at INTEGER,
                embedding TEXT,
                embedding_blob BLOB,
                okf_type TEXT,
                ontology_checked_at INTEGER,
                heal_checked_at INTEGER,
                lifecycle_status TEXT NOT NULL DEFAULT 'stable',
                stale_after INTEGER,
                generated_by TEXT,
                last_verified_at INTEGER,
                last_verified_by TEXT,
                okf_sources TEXT,
                okf_verified TEXT,
                okf_usage_window TEXT,
                embedding_failed_at INTEGER,
                embedding_failure_kind TEXT,
                embedding_attempts INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE curated_entities (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                entity_type TEXT NOT NULL DEFAULT 'concept',
                summary TEXT NOT NULL DEFAULT '',
                summary_embedding BLOB,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                deleted_at INTEGER
            );
            CREATE TABLE curated_agent_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                client TEXT NOT NULL,
                tool TEXT NOT NULL,
                operation TEXT NOT NULL CHECK(operation IN ('read','write')),
                entity_id TEXT,
                summary TEXT,
                created_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE TABLE llm_wiki_tasks (
                id TEXT PRIMARY KEY,
                entity_id TEXT NOT NULL,
                description TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                priority INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                resolved_at INTEGER,
                deleted_at INTEGER
            );
            CREATE TABLE llm_wiki_events (
                id TEXT PRIMARY KEY,
                entity_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                summary TEXT,
                related_entry_id TEXT,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE llm_wiki_edges (
                id TEXT PRIMARY KEY,
                entity_id TEXT NOT NULL,
                source_id TEXT NOT NULL,
                target_id TEXT NOT NULL,
                edge_type TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                UNIQUE(entity_id, source_id, target_id, edge_type)
            );
            CREATE TABLE llm_wiki_outbox (
                id TEXT PRIMARY KEY,
                entity_id TEXT NOT NULL,
                table_name TEXT NOT NULL,
                record_id TEXT NOT NULL,
                operation TEXT NOT NULL,
                payload TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE documents (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                path TEXT NOT NULL UNIQUE,
                hash TEXT NOT NULL,
                tier TEXT NOT NULL CHECK(tier IN ('user_doc', 'wiki')),
                folder_rules_id INTEGER,
                last_indexed INTEGER,
                status TEXT NOT NULL DEFAULT 'pending'
                    CHECK(status IN ('pending', 'pending_reindex', 'indexed', 'error', 'orphaned'))
            );
            CREATE TABLE chunks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                doc_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
                chunk_text TEXT NOT NULL,
                position INTEGER NOT NULL,
                start_line INTEGER NOT NULL DEFAULT 1,
                end_line INTEGER NOT NULL DEFAULT 1,
                symbol_name TEXT,
                strategy TEXT NOT NULL DEFAULT 'prose',
                defined_symbol TEXT,
                entity_id TEXT
            );
            CREATE TABLE embeddings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chunk_id INTEGER NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
                vector BLOB NOT NULL
            );
            INSERT INTO curated_entities VALUES
              ('ent-1', 'Farmhouse', 'concept', '', NULL, 1756000000, 1756000000, NULL);
            INSERT INTO llm_wiki_entries
              (id, entity_id, title, body, tags, confidence, source_type, source_ref,
               created_at, updated_at, deleted_at) VALUES
              ('w1','ent-1','Repo Layout','The vault stores immutable documents.','[]','confirmed','user_stated','{"proposal_id":null,"evidence":[]}',1756000000000,1756000000000,NULL),
              ('w2','ent-1','Archived note','older body','[]','inferred','user_stated','x',1756000000000,1756000000000,1756000000001);
            INSERT INTO documents (path, hash, tier, status) VALUES
              ('src/main.rs', 'h1', 'user_doc', 'indexed'),
              ('docs/readme.md', 'h2', 'user_doc', 'indexed');
            INSERT INTO chunks (doc_id, chunk_text, position, start_line, end_line, symbol_name, strategy, entity_id) VALUES
              (1, 'fn main() {}', 0, 1, 1, 'main', 'ast_symbols', 'tier_working'),
              (2, 'plain prose chunk', 0, 1, 1, NULL, 'proximity', 'tier_working');
            INSERT INTO embeddings (chunk_id, vector) VALUES
              (1, x'0000803F000000000000000000000000000000000000000000000000000000'),
              (2, x'0000003F000000000000000000000000000000000000000000000000000000');
            "#,
        )
        .unwrap();
        conn
    }

    #[test]
    fn rank_wiki_entries_skips_deleted_and_ranks() {
        let dir = tempfile::TempDir::new().unwrap();
        let conn = seed_file_db(dir.path());
        let hits = rank_wiki_entries(&conn, "vault documents", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["id"], "w1");
    }

    /// ToolDispatchContext over the shared file fixture. The embed stub env
    /// (CURATED_EMBED_STUB=constant8) keeps `embed_batch` deterministic and
    /// network-free; `Local` profile is the repo default.
    fn test_ctx(conn: Connection, dir: &std::path::Path) -> ToolDispatchContext {
        std::env::set_var("CURATED_EMBED_STUB", "constant8"); // mandated stub
        ToolDispatchContext {
            conn: Arc::new(Mutex::new(conn)),
            profile: EmbedProfile::default(),
            vault_dir: Some(dir.to_path_buf()),
            client: "test".into(),
            db_path: dir.join("brain.db"),
            rw_conn: Arc::new(Mutex::new(None)),
        }
    }

    #[tokio::test]
    async fn recall_context_returns_wiki_first() {
        let dir = tempfile::TempDir::new().unwrap();
        let conn = seed_file_db(dir.path());
        let ctx = test_ctx(conn, dir.path());
        let v = dispatch_curated_recall_context(
            &ctx,
            CuratedRecallContextParams {
                query: "vault documents".into(),
                limit_wiki: Some(3),
                limit_code: Some(3),
            },
        )
        .await
        .unwrap();
        assert!(v["wiki_entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == serde_json::json!("w1")));
        assert_eq!(v["query"], "vault documents");
    }

    #[tokio::test]
    async fn get_entry_requires_topic_or_entity() {
        let dir = tempfile::TempDir::new().unwrap();
        let conn = seed_file_db(dir.path());
        let ctx = test_ctx(conn, dir.path());
        assert!(dispatch_curated_get_wiki_entry(
            &ctx,
            CuratedGetWikiEntryParams {
                topic: None,
                entity_id: None
            }
        )
        .await
        .is_err());
        let v = dispatch_curated_get_wiki_entry(
            &ctx,
            CuratedGetWikiEntryParams {
                topic: Some("Repo Layout".into()),
                entity_id: None,
            },
        )
        .await
        .unwrap();
        assert!(v["full_text"]
            .as_str()
            .unwrap()
            .contains("immutable documents"));
    }

    #[tokio::test]
    async fn get_entry_entity_id_wins_over_topic() {
        // spec §6: both supplied -> entity_id takes precedence
        let dir = tempfile::TempDir::new().unwrap();
        let conn = seed_file_db(dir.path());
        let ctx = test_ctx(conn, dir.path());
        let v = dispatch_curated_get_wiki_entry(
            &ctx,
            CuratedGetWikiEntryParams {
                topic: Some("nonexistent-topic-xyz".into()),
                entity_id: Some("w1".into()),
            },
        )
        .await
        .unwrap();
        assert!(v["full_text"]
            .as_str()
            .unwrap()
            .contains("immutable documents"));
    }

    #[tokio::test]
    async fn search_code_filters_ast_only() {
        let dir = tempfile::TempDir::new().unwrap();
        let conn = seed_file_db(dir.path());
        let ctx = test_ctx(conn, dir.path());
        let v = dispatch_curated_search_code(
            &ctx,
            CuratedSearchCodeParams {
                query: "main".into(),
                limit: Some(5),
                symbol: Some("main".into()),
            },
        )
        .await
        .unwrap();
        for c in v["code_chunks"].as_array().unwrap() {
            assert!(c["strategy"].as_str().unwrap().starts_with("ast_"));
        }
    }

    #[tokio::test]
    async fn with_rw_errors_when_db_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let conn = seed_file_db(dir.path());
        let mut ctx = test_ctx(conn, dir.path());
        ctx.db_path = std::path::PathBuf::from("/nonexistent/brain.db");
        // with_rw is async — awaited directly (no run_sync helper).
        let err = ctx.with_rw(|_c| Ok(())).await;
        assert!(err.is_err()); // never creates the brain file
        assert!(
            !std::path::Path::new("/nonexistent/brain.db").exists(),
            "with_rw must never create the DB file"
        );
    }

    #[tokio::test]
    async fn with_rw_opens_existing_db_readwrite() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = dir.path().join("brain.db");
        Connection::open(&db)
            .unwrap()
            .execute_batch("CREATE TABLE t(x);")
            .unwrap();
        let conn = seed_file_db(dir.path());
        let mut ctx = test_ctx(conn, dir.path());
        ctx.db_path = db;
        ctx.with_rw(|c| {
            c.execute("INSERT INTO t VALUES (1)", [])
                .map_err(|e| anyhow::anyhow!(e))?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn add_wisdom_inserts_user_stated_wisdom() {
        let dir = tempfile::TempDir::new().unwrap();
        let conn = seed_file_db(dir.path());
        let mut ctx = test_ctx(conn, dir.path());
        ctx.db_path = dir.path().join("brain.db"); // real RW path over the seeded file
        let v = dispatch_curated_add_wisdom(
            &ctx,
            CuratedAddWisdomParams {
                entity_id: "ent-1".into(),
                body: "Agent learned: deploy scripts need sudo.".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(v["source_type"], "user_stated");
        assert!(v["id"].as_str().unwrap().starts_with("fact_")); // id prefix is storage-level
        // Audit row landed (fail-closed path wrote a 'write' row).
        let audit_count: i64 = ctx
            .with_rw(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM curated_agent_log WHERE tool = 'curated_add_wisdom'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(audit_count, 1);
    }

    #[tokio::test]
    async fn update_wisdom_returns_reloaded_entry() {
        let dir = tempfile::TempDir::new().unwrap();
        let conn = seed_file_db(dir.path());
        let mut ctx = test_ctx(conn, dir.path());
        ctx.db_path = dir.path().join("brain.db");
        // Update the seeded entry w1 under entity ent-1.
        let v = dispatch_curated_update_wisdom(
            &ctx,
            CuratedUpdateWisdomParams {
                entity_id: "ent-1".into(),
                wisdom_id: "w1".into(),
                body: "updated body v2".into(),
            },
        )
        .await
        .unwrap();
        // Reloaded from the DB — not echoed from the request.
        assert_eq!(v["body"], "updated body v2");
        assert_eq!(v["id"], "w1");
        assert_eq!(v["entity_id"], "ent-1");
    }

    #[tokio::test]
    async fn archive_wisdom_soft_deletes() {
        let dir = tempfile::TempDir::new().unwrap();
        let conn = seed_file_db(dir.path());
        let mut ctx = test_ctx(conn, dir.path());
        ctx.db_path = dir.path().join("brain.db");
        let v = dispatch_curated_archive_wisdom(
            &ctx,
            CuratedArchiveWisdomParams {
                entity_id: "ent-1".into(),
                wisdom_id: "w1".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(v["archived"], true);
        assert_eq!(v["wisdom_id"], "w1");
        // Verify via the RO conn: deleted_at set (ms epoch) + live count dropped.
        let (deleted_at, live_count): (Option<i64>, i64) = {
            let guard = ctx.conn.lock().unwrap();
            (
                guard
                    .query_row(
                        "SELECT deleted_at FROM llm_wiki_entries WHERE id = 'w1'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap(),
                guard
                    .query_row(
                        "SELECT COUNT(*) FROM llm_wiki_entries WHERE deleted_at IS NULL",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap(),
            )
        };
        assert!(deleted_at.is_some(), "w1 must carry a deleted_at ms stamp");
        assert_eq!(live_count, 0, "w2 was seeded pre-archived; after archiving w1 no live rows remain");
    }

    #[tokio::test]
    async fn write_fails_when_entity_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let conn = seed_file_db(dir.path());
        let mut ctx = test_ctx(conn, dir.path());
        ctx.db_path = dir.path().join("brain.db");
        let err = dispatch_curated_add_wisdom(
            &ctx,
            CuratedAddWisdomParams {
                entity_id: "ent-gone".into(),
                body: "orphan wisdom".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("ent-gone")
                || err.to_string().to_lowercase().contains("not found")
                || err.to_string().to_lowercase().contains("inactive"),
            "core must bail on inactive/missing entity; got: {err}"
        );
    }

    #[tokio::test]
    async fn curated_call_fails_when_audit_log_unwritable() {
        // spec §9 log-failure test: DROP TABLE curated_agent_log via a THIRD
        // direct Connection handle (the RO conn cannot write DDL), then the
        // curated write must fail (fail-closed audit).
        let dir = tempfile::TempDir::new().unwrap();
        let conn = seed_file_db(dir.path());
        let mut ctx = test_ctx(conn, dir.path());
        ctx.db_path = dir.path().join("brain.db");
        {
            let third = Connection::open(dir.path().join("brain.db")).unwrap();
            third.execute_batch("DROP TABLE curated_agent_log;").unwrap();
        }
        let err = dispatch_curated_add_wisdom(
            &ctx,
            CuratedAddWisdomParams {
                entity_id: "ent-1".into(),
                body: "should fail on audit write".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("audit"),
            "error must trace to the audit insert; got: {err}"
        );
    }
}
