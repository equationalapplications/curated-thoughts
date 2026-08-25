use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::json;
use tauri_app_lib::db::proposals::{self, ProposalDetail};
use tauri_app_lib::embedder::{embed_one, EmbedProfile};
use tauri_app_lib::retrieval::{self, BrainPaths};
use tauri_app_lib::search::{bytes_to_f32, cosine_similarity};

pub struct Brain {
    pub paths: BrainPaths,
}

pub fn resolve() -> Result<Brain> {
    let paths = retrieval::resolve_brain_paths();
    if !paths.db_path.exists() {
        bail!(
            "brain.db not found at {} — run ingest first",
            paths.db_path.display()
        );
    }
    Ok(Brain { paths })
}

pub fn open_ro(brain: &Brain) -> Result<rusqlite::Connection> {
    Ok(retrieval::open_brain_readonly(&brain.paths.db_path)?)
}

pub fn open_rw(brain: &Brain) -> Result<rusqlite::Connection> {
    rusqlite::Connection::open_with_flags(
        &brain.paths.db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .context("opening brain.db read-write")
}

/// Exit code contract: 0 ok, 1 error, 2 no results.
pub const EXIT_NO_RESULTS: i32 = 2;

pub fn print_json<T: Serialize>(v: &T) {
    println!("{}", serde_json::to_string_pretty(v).expect("serialize"));
}

/// One row of `ct proposals list` output.
#[derive(Debug, Clone, Serialize)]
pub struct PendingProposal {
    pub id: String,
    pub source_doc_path: Option<String>,
    pub created_at: i64,
    pub item_count: usize,
}

/// Pending proposals, oldest first. item_count comes from the same
/// `get_proposal_detail` call the GUI renders (N+1 is fine at pending
/// volumes); source_doc_path is the first source doc, when any.
pub fn list_pending_proposals(conn: &Connection) -> Result<Vec<PendingProposal>> {
    let mut stmt = conn.prepare(
        "SELECT id, created_at
         FROM curated_proposals
         WHERE status = 'pending'
         ORDER BY created_at",
    )?;
    let rows: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, created_at) in rows {
        let detail = proposals::get_proposal_detail(conn, &id)?
            .with_context(|| format!("pending proposal {id} vanished mid-list"))?;
        out.push(PendingProposal {
            id,
            source_doc_path: detail.source_doc_paths.first().cloned(),
            created_at,
            item_count: detail.items.len(),
        });
    }
    Ok(out)
}

/// Full proposal JSON for `ct proposals show` — delegates entirely to
/// `get_proposal_detail`; `None` means unknown id.
pub fn show_proposal(conn: &Connection, id: &str) -> Result<Option<ProposalDetail>> {
    proposals::get_proposal_detail(conn, id)
}

// ---------------------------------------------------------------------------
// Shared query helpers (extracted from tools/src/bin/curated_thoughts_mcp.rs;
// SQL kept verbatim so sidecar behavior is unchanged).
// ---------------------------------------------------------------------------

/// One cosine-ranked code chunk from the chunks-JOIN-embeddings recall leg.
#[derive(Debug, Clone, Serialize)]
pub struct ScoredChunk {
    pub doc_path: String,
    pub chunk_text: String,
    pub score: f32,
    pub symbol_name: Option<String>,
    pub entity_id: String,
}

/// Both recall legs for a coding-task query.
#[derive(Debug, Clone, Serialize)]
pub struct RecallLegs {
    pub wiki: Vec<serde_json::Value>,
    pub chunks: Vec<ScoredChunk>,
}

/// Full ranked row as consumed by the MCP sidecar's JSON responses (which
/// expose id/lines/strategy fields beyond what `ScoredChunk` carries).
#[derive(Debug, Clone)]
pub struct RankedChunkRow {
    pub id: i64,
    pub text: String,
    pub doc_path: String,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub symbol_name: Option<String>,
    /// Second strategy-ish column (`c.strategy` in curated_search_code).
    pub language: Option<String>,
    pub entity_id: String,
    pub score: f32,
}

/// Coerce a stored `updated_at` cell into an integer epoch ranking key.
///
/// The live schema declares `updated_at INTEGER`, but desktop-app writes and
/// older sidecars may have produced TEXT-form values; NULL is also tolerated.
/// Any value that cannot be coerced ranks as 0 — acceptable for a sort key.
pub fn coerce_updated_at(value: Option<rusqlite::types::Value>) -> i64 {
    match value {
        Some(rusqlite::types::Value::Integer(i)) => i,
        Some(rusqlite::types::Value::Text(s)) => s.parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

/// Wisdom layer: llm_wiki_entries is the librarian's synthesis destination. It
/// has no embeddings populated yet, so rank entries by title/body keyword
/// overlap with the query terms (BM25-lite); when the table is empty this leg
/// returns [] without error. Candidates are aggregated across all query terms,
/// ranked by term overlap then confidence/updated_at, and truncated to
/// limit_wiki only at the end.
pub fn rank_wiki_entries(
    conn: &Connection,
    query: &str,
    limit_wiki: usize,
) -> Result<Vec<serde_json::Value>> {
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
    let mut candidates: HashMap<String, (usize, serde_json::Value, String, i64)> = HashMap::new();
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
/// sorted by score descending and truncated to `limit`.
pub fn fetch_ranked_chunks(
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

/// Recall-leg SQL over real chunker strategies (ast_*). Vectors live in the
/// separate embeddings table. With the AST predicate appended this matches the
/// MCP sidecar's `curated_recall_context` / `curated_search_code` leg exactly.
pub const RECALL_CHUNKS_SQL_BASE: &str = "
    SELECT c.id, c.chunk_text, e.vector, d.path, c.start_line, c.end_line,
           c.symbol_name, c.strategy, c.entity_id
    FROM chunks c
    JOIN documents d ON c.doc_id = d.id
    JOIN embeddings e ON e.chunk_id = c.id
    WHERE d.status = 'indexed'
";
pub const RECALL_CHUNKS_AST_FILTER: &str = " AND c.strategy LIKE 'ast%'";

/// Embed the query and run the chunks-JOIN-embeddings cosine leg.
///
/// `ast_only=true` reproduces the recall leg's `strategy LIKE 'ast%'` filter
/// (also used by `ct code`); `ast_only=false` returns every indexed chunk.
pub fn recall_chunks(
    conn: &Connection,
    profile: &EmbedProfile,
    query: &str,
    limit: usize,
    ast_only: bool,
) -> Result<Vec<ScoredChunk>> {
    let query_embedding = embed_one(profile, query.to_string()).context("failed to embed query")?;
    let mut sql = RECALL_CHUNKS_SQL_BASE.to_string();
    if ast_only {
        sql.push_str(RECALL_CHUNKS_AST_FILTER);
    }
    let rows = fetch_ranked_chunks(conn, &sql, &[], &query_embedding, limit)?;
    Ok(rows
        .into_iter()
        .map(|r| ScoredChunk {
            doc_path: r.doc_path,
            chunk_text: r.text,
            score: r.score,
            symbol_name: r.symbol_name,
            entity_id: r.entity_id,
        })
        .collect())
}

/// Best-effort canonicalization for dedup keys; falls back to the input.
fn canonicalize_best_effort(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.to_string())
}

/// Result-level dedup on (canonicalized doc_path, chunk_text). Ingest already
/// enforces unique (doc_id, content_hash); this catches repo/symlink
/// duplicates that surface as distinct documents with identical content.
pub fn dedup_chunks(chunks: Vec<ScoredChunk>) -> Vec<ScoredChunk> {
    use std::collections::HashSet;
    let mut seen = HashSet::new();
    chunks
        .into_iter()
        .filter(|c| seen.insert((canonicalize_best_effort(&c.doc_path), c.chunk_text.clone())))
        .collect()
}

/// Clamp k the same way the retrieval facade does (`limit.clamp(1, 50)`).
pub fn clamp_limit(k: usize) -> usize {
    k.clamp(1, 50)
}

/// Shared plumbing for search/code/recall: resolve brain, load the embed
/// profile, embed + rank via recall_chunks, dedupe, and render.
fn run_query(
    query: &str,
    k: usize,
    ast_only: bool,
) -> Result<std::result::Result<Vec<ScoredChunk>, ()>> {
    if query.trim().is_empty() {
        return Ok(Err(()));
    }
    let brain = resolve()?;
    let conn = open_ro(&brain)?;
    let config_path = brain.paths.config_path.clone();
    let profile = retrieval::load_embed_profile(&config_path)
        .context("loading embed profile from vault config.json — point CURATED_BRAIN_DIR at the folder containing config.json and brain.db")?;
    let chunks = dedup_chunks(recall_chunks(
        &conn,
        &profile,
        query,
        clamp_limit(k),
        ast_only,
    )?);
    Ok(if chunks.is_empty() {
        Err(())
    } else {
        Ok(chunks)
    })
}

pub fn search_cmd(query: &str, k: usize, json_mode: bool) -> Result<i32> {
    cmd_output(run_query(query, k, false)?, json_mode)
}

/// `ct code`: semantic search restricted to ast% strategy chunks.
pub fn code_cmd(query: &str, k: usize, json_mode: bool) -> Result<i32> {
    cmd_output(run_query(query, k, true)?, json_mode)
}

/// `ct recall`: chunk results plus the wiki ranking leg.
pub fn recall_cmd(query: &str, k: usize, json_mode: bool) -> Result<i32> {
    match run_query(query, k, false)? {
        Err(()) => Ok(EXIT_NO_RESULTS),
        Ok(chunks) => {
            let brain = resolve()?;
            let conn = open_ro(&brain)?;
            let wiki = rank_wiki_entries(&conn, query, 5)?;
            if json_mode {
                print_json(&json!({
                    "results": chunks,
                    "wiki": wiki,
                }));
            } else {
                for c in &chunks {
                    println!(
                        "{:.4} {}: {}",
                        c.score,
                        c.doc_path,
                        first_line(&c.chunk_text)
                    );
                }
                if !wiki.is_empty() {
                    println!("--- wiki ---");
                    for w in &wiki {
                        let title = w["title"].as_str().unwrap_or("");
                        println!("wiki: {title}");
                    }
                }
            }
            Ok(0)
        }
    }
}

fn cmd_output(chunks: std::result::Result<Vec<ScoredChunk>, ()>, json_mode: bool) -> Result<i32> {
    let chunks = match chunks {
        Ok(c) => c,
        Err(()) => return Ok(EXIT_NO_RESULTS),
    };
    if json_mode {
        print_json(&json!({ "results": chunks }));
    } else {
        for c in &chunks {
            println!(
                "{:.4} {}: {}",
                c.score,
                c.doc_path,
                first_line(&c.chunk_text)
            );
        }
    }
    Ok(0)
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

// ---------------------------------------------------------------------------
// Task 5: ct graph traversal + ct wiki get/list
// ---------------------------------------------------------------------------

/// Hard neighbor cap shared with the MCP sidecar's graph tool.
pub const GRAPH_MAX_NEIGHBORS: usize = 200;

/// Traversal direction for `ct graph` (validated at the clap parse level via
/// `value_enum`, mapping onto the graph module's callees/callers/both CTEs).
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum GraphDir {
    Callees,
    Callers,
    Both,
}

/// Serialize one graph traversal as
/// `{"root": {chunk_id, entity_id}, "neighbors": [{chunk_id, depth, rel_type,
/// entity_id}], "truncated": bool}`, joining `chunks.entity_id` for each
/// NeighborRow.chunk_id.
pub fn graph_json(
    conn: &Connection,
    root_chunk_id: i64,
    root_entity_id: &str,
    neighbors: &[tauri_app_lib::graph::NeighborRow],
) -> serde_json::Value {
    let entity_of = |chunk_id: i64| -> String {
        conn.query_row(
            "SELECT COALESCE(entity_id, '') FROM chunks WHERE id = ?1",
            rusqlite::params![chunk_id],
            |r| r.get::<_, String>(0),
        )
        .unwrap_or_default()
    };
    let truncated = neighbors.len() > GRAPH_MAX_NEIGHBORS;
    let listed: Vec<serde_json::Value> = neighbors
        .iter()
        .take(GRAPH_MAX_NEIGHBORS)
        .map(|n| {
            json!({
                "chunk_id": n.chunk_id,
                "depth": n.depth,
                "rel_type": n.rel_type,
                "entity_id": entity_of(n.chunk_id),
            })
        })
        .collect();
    json!({
        "root": { "chunk_id": root_chunk_id, "entity_id": root_entity_id },
        "neighbors": listed,
        "truncated": truncated,
    })
}

/// `ct graph`: resolve the symbol, run the traversal, render JSON or text.
pub fn graph_cmd(symbol: &str, dir: GraphDir, hops: u32, json_mode: bool) -> Result<i32> {
    let brain = resolve()?;
    let conn = open_ro(&brain)?;
    let (root_chunk_id, root_entity_id) = match resolve_symbol(&conn, symbol)? {
        Some(pair) => pair,
        None => {
            eprintln!("symbol not found: {symbol}");
            return Ok(EXIT_NO_RESULTS);
        }
    };
    let neighbors = match dir {
        GraphDir::Callees => {
            tauri_app_lib::graph::get_callees(&conn, root_chunk_id, &root_entity_id, hops)?
        }
        GraphDir::Callers => {
            tauri_app_lib::graph::get_callers(&conn, root_chunk_id, &root_entity_id, hops)?
        }
        GraphDir::Both => {
            tauri_app_lib::graph::get_both(&conn, root_chunk_id, &root_entity_id, hops)?
        }
    };
    if json_mode {
        print_json(&graph_json(
            &conn,
            root_chunk_id,
            &root_entity_id,
            &neighbors,
        ));
    } else {
        println!("root: {} ({root_entity_id})", symbol.trim().to_lowercase());
        for n in neighbors.iter().take(GRAPH_MAX_NEIGHBORS) {
            println!(
                "{:>2} {:<8} {} {}",
                n.depth,
                n.rel_type,
                n.chunk_id,
                entity_lookup(&conn, n.chunk_id)
            );
        }
    }
    Ok(0)
}

fn entity_lookup(conn: &Connection, chunk_id: i64) -> String {
    conn.query_row(
        "SELECT COALESCE(entity_id, '') FROM chunks WHERE id = ?1",
        rusqlite::params![chunk_id],
        |r| r.get::<_, String>(0),
    )
    .unwrap_or_default()
}

/// One wiki row as a JSON object (`ct wiki list --json`, eval-C2 gap).
pub fn wiki_rows(conn: &Connection) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT id, entity_id, title, body, confidence, updated_at
         FROM llm_wiki_entries
         WHERE deleted_at IS NULL
         ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(json!({
            "id": row.get::<_, String>(0)?,
            "entity_id": row.get::<_, Option<String>>(1)?,
            "title": row.get::<_, Option<String>>(2)?,
            "body": row.get::<_, Option<String>>(3)?,
            "confidence": row.get::<_, Option<String>>(4)?,
            "updated_at": row.get::<_, Option<rusqlite::types::Value>>(5)
                .ok()
                .and_then(|v| v)
                .map(|v| coerce_updated_at(Some(v))),
        }))
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn wiki_text_line(w: &serde_json::Value) -> String {
    format!(
        "{}  {}",
        w["id"].as_str().unwrap_or(""),
        w["title"].as_str().unwrap_or("")
    )
}

/// `ct wiki list`: all non-deleted entries.
pub fn wiki_list_cmd(json_mode: bool) -> Result<i32> {
    let brain = resolve()?;
    let conn = open_ro(&brain)?;
    let rows = wiki_rows(&conn)?;
    if rows.is_empty() {
        eprintln!("no wiki entries");
        return Ok(EXIT_NO_RESULTS);
    }
    if json_mode {
        print_json(&rows);
    } else {
        for w in &rows {
            println!("{}", wiki_text_line(w));
        }
    }
    Ok(0)
}

/// `ct wiki get <entity_id>`: full row(s), body included.
pub fn wiki_get_cmd(entity_id: &str, json_mode: bool) -> Result<i32> {
    let brain = resolve()?;
    let conn = open_ro(&brain)?;
    let rows: Vec<serde_json::Value> = wiki_rows(&conn)?
        .into_iter()
        .filter(|w| w["entity_id"] == *entity_id)
        .collect();
    if rows.is_empty() {
        eprintln!("wiki entry not found: {entity_id}");
        return Ok(EXIT_NO_RESULTS);
    }
    if json_mode {
        print_json(&rows);
    } else {
        for w in &rows {
            println!("{}", serde_json::to_string_pretty(w).expect("serialize"));
        }
    }
    Ok(0)
}

/// Resolve a user-supplied symbol name to its definition chunk.
///
/// The name is normalized (trim + lowercase, matching how the linker stores
/// `defined_symbol`) and resolution prefers rows where `defined_symbol IS NOT
/// NULL`, falling back to `symbol_name`; lowest chunk id breaks ties. Returns
/// `(chunk_id, entity_id)` or `None` when nothing matches.
pub fn resolve_symbol(conn: &Connection, symbol: &str) -> Result<Option<(i64, String)>> {
    use rusqlite::OptionalExtension;
    let normalized = symbol.trim().to_lowercase();
    conn.query_row(
        "SELECT c.id, c.entity_id FROM chunks c
         WHERE c.defined_symbol = ?1 OR (c.defined_symbol IS NULL AND c.symbol_name = ?1)
         ORDER BY CASE WHEN c.defined_symbol IS NOT NULL THEN 0 ELSE 1 END, c.id
         LIMIT 1",
        rusqlite::params![normalized],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            ))
        },
    )
    .optional()
    .context("resolve symbol query")
}
