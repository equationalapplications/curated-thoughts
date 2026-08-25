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
    retrieval::open_brain_readonly(&brain.paths.db_path)
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
) -> Result<Option<(rusqlite::Connection, Vec<ScoredChunk>)>> {
    if query.trim().is_empty() {
        return Ok(None);
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
        None
    } else {
        Some((conn, chunks))
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
    // None means no chunk results (exit 2). The connection is returned so the
    // wiki ranking leg reuses it instead of reopening the database.
    let Some((conn, chunks)) = run_query(query, k, false)? else {
        return Ok(EXIT_NO_RESULTS);
    };
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

fn cmd_output(
    chunks: Option<(rusqlite::Connection, Vec<ScoredChunk>)>,
    json_mode: bool,
) -> Result<i32> {
    let (_, chunks) = match chunks {
        Some(c) => c,
        None => return Ok(EXIT_NO_RESULTS),
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

// ---------------------------------------------------------------------------
// Write operations (Task 7): extracted verbatim from the ingest_vault_once /
// run_librarian_once / approve_pending_proposals bins so both the bins (thin
// mains) and `ct` can call them. Bin behavior is unchanged.
// ---------------------------------------------------------------------------

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tauri_app_lib::chunker::should_ingest_extension;
use tauri_app_lib::db::commit::{resolve_proposal, ResolveOptions};
use tauri_app_lib::db::connection::AppDb;
use tauri_app_lib::db::proposals::{get_proposal_detail, ItemDecision, ItemDecisionKind};
use tauri_app_lib::indexer::linker::run_linker;
use tauri_app_lib::vault::VaultConfig;
use tauri_app_lib::{entity_id_for_path, ingest_document_with_vault_root};

/// Directory names never ingested (build artifacts, deps, VCS internals).
const EXCLUDED_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "dist-newstyle",
    ".git",
    ".github",
    ".next",
    ".turbo",
    ".cache",
    "coverage",
    "build",
    "out",
    ".venv",
    "venv",
    "__pycache__",
    ".idea",
    ".vscode",
    ".fastembed_cache",
];

fn is_excluded_dir(dir_name: &str) -> bool {
    EXCLUDED_DIRS.contains(&dir_name)
}

/// File-name patterns never ingested: machine-generated dependency manifests
/// and generated schemas. The chunker bounds chunk size, so this is not about
/// file length — these files carry no retrieval value and just burn embedding
/// API calls (all 20 failures in the Aug 24 full-corpus run were these).
const EXCLUDED_FILE_NAMES: &[&str] = &[
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "Cargo.lock",
    "poetry.lock",
    "uv.lock",
    "CHANGELOG.md",
    "CHANGELOG.md.generated", // commitizen-style generated changelogs
];

/// Path segments (matched anywhere in the relative path) that mark generated
/// machine output rather than authored knowledge.
const EXCLUDED_PATH_SEGMENTS: &[&str] = &["drizzle/meta/", "gen/schemas/"];

fn is_excluded_file(path: &Path) -> bool {
    if let Some(name) = path.file_name() {
        let name = name.to_string_lossy();
        if EXCLUDED_FILE_NAMES.contains(&name.as_ref()) {
            return true;
        }
    }
    let p = path.to_string_lossy();
    EXCLUDED_PATH_SEGMENTS.iter().any(|seg| p.contains(seg))
}

/// Collect files from a directory tree. `follow_symlinked_doc_dirs` enables
/// following symlinked directories whose parent is exactly
/// `<vault_root>/documents` (the staging contract); nested symlinks and
/// symlinks to files are never followed. Traversal errors are returned so an
/// unreadable path can't silently shrink the corpus.
fn collect_files(
    root: &Path,
    follow_symlinked_doc_dirs: bool,
    out: &mut Vec<PathBuf>,
    errors: &mut Vec<String>,
) {
    let walker = walkdir::WalkDir::new(root).follow_links(false);
    let it = walker.into_iter().filter_entry(|e| {
        // Skip excluded dirs by name at any depth.
        if e.file_type().is_dir() {
            if let Some(name) = e.path().file_name() {
                return !is_excluded_dir(&name.to_string_lossy());
            }
        }
        true
    });
    for entry in it {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                errors.push(format!("traversal: {e}"));
                continue;
            }
        };
        let p = entry.path();
        if entry.file_type().is_file() && is_excluded_file(p) {
            continue;
        }
        let ft = entry.file_type();
        if ft.is_file()
            && p.extension()
                .map(|e| should_ingest_extension(&e.to_string_lossy()))
                .unwrap_or(false)
        {
            out.push(p.to_path_buf());
        } else if follow_symlinked_doc_dirs && ft.is_symlink() {
            // Only follow symlinks that are DIRECT children of
            // <root>/documents, whose names aren't excluded, and whose target
            // is a directory. Never follow file symlinks or nested ones.
            let parent_is_documents = p
                .parent()
                .map(|par| par.file_name().map(|n| n == "documents").unwrap_or(false))
                .unwrap_or(false)
                && entry.depth() == 1;
            let name_excluded = p
                .file_name()
                .map(|n| is_excluded_dir(&n.to_string_lossy()))
                .unwrap_or(false);
            if !parent_is_documents || name_excluded {
                continue;
            }
            match std::fs::canonicalize(p) {
                Ok(target) if target.is_dir() => {
                    // Recurse into the resolved target with symlink-following
                    // OFF, so nested symlinks inside are never descended into.
                    collect_files(&target, false, out, errors)
                }
                Ok(_) => eprintln!(
                    "warn: symlink {} does not point at a directory, skipping",
                    p.display()
                ),
                Err(e) => eprintln!("warn: broken symlink {}, skipping: {e}", p.display()),
            }
        }
    }
}

/// Full ingest_vault_once flow: resolve brain paths + embed profile, open the
/// brain DB, walk the vault honoring the exclusion rules, ingest every
/// ingestible file, then run the linker over each touched entity. Extracted
/// from `ingest_vault_once.rs`; behavior identical to the original bin main.
pub fn ingest_run() -> Result<()> {
    let paths_b = retrieval::resolve_brain_paths();
    let profile =
        retrieval::load_embed_profile(&paths_b.config_path).context("read embed profile")?;
    let db = AppDb::open(&paths_b.db_path).context("open brain database")?;
    let conn = &db.0;

    let config = VaultConfig::new(paths_b.config_path.clone());
    let vault_root = config
        .vault_root()
        .context("read vault root")?
        .ok_or_else(|| anyhow::anyhow!("vault root missing"))?;
    let vault_root = vault_root.canonicalize().unwrap_or(vault_root);

    let mut files = Vec::new();
    let mut walk_errors = Vec::new();
    collect_files(&vault_root, true, &mut files, &mut walk_errors);
    files.sort();
    files.dedup();

    // Traversal errors count as failures so an unreadable path can't make a
    // partial run look complete.
    let mut failed = walk_errors.len();
    for e in &walk_errors {
        eprintln!("warn: {e}");
    }
    println!(
        "ingesting {} file(s) from {}",
        files.len(),
        vault_root.display()
    );

    let vault_root_str = vault_root.to_str().unwrap();
    let mut entity_ids = HashSet::new();
    for (i, f) in files.iter().enumerate() {
        match ingest_document_with_vault_root(
            conn,
            &profile,
            f.to_str().unwrap(),
            true,
            Some(vault_root_str),
        ) {
            Ok(_) => {
                entity_ids.insert(entity_id_for_path(
                    f.to_str().unwrap(),
                    Some(vault_root_str),
                ));
                println!("[{}/{}] ok: {}", i + 1, files.len(), f.display());
            }
            Err(e) => {
                failed += 1;
                eprintln!("[{}/{}] FAILED {}: {}", i + 1, files.len(), f.display(), e);
                let mut src = e.source();
                while let Some(s) = src {
                    eprintln!("    caused by: {s}");
                    src = s.source();
                }
            }
        }
    }

    for entity_id in &entity_ids {
        if let Err(e) = run_linker(conn, entity_id, 0) {
            eprintln!("[linker] {}: {}", entity_id, e);
        }
    }
    println!(
        "done: {} docs, {} entities, {} failed",
        files.len(),
        entity_ids.len(),
        failed
    );
    Ok(())
}

/// Full run_librarian_once flow over already-ingested documents with the
/// given fallback model (bin default: "llama3.2:3b"; config overrides it in
/// sidecar mode). Extracted from `run_librarian_once.rs`.
pub fn librarian_run(model: &str, force: bool) -> Result<()> {
    let paths = retrieval::resolve_brain_paths();
    if !paths.db_path.is_file() {
        anyhow::bail!(
            "brain database not found at {} — run the app (or ingest_vault_once) first",
            paths.db_path.display()
        );
    }
    // Honor split CURATED_BRAIN_DB / CURATED_BRAIN_CONFIG: resolve the vault
    // root from the resolved config path, not from db_path's parent.
    let mut db = AppDb::open_with_config(&paths.db_path, &paths.config_path)
        .with_context(|| format!("open brain database {}", paths.db_path.display()))?;
    // Resolve errors.log's parent directory the same way write_synthesis_error does
    // (vault root derived from the config path), not from db_path's parent, so
    // surface-detection stays correct under non-default brain-dir layouts.
    let error_log_dir = paths.config_path.parent();
    librarian_run_on(&mut db.0, error_log_dir, model, force)
}

/// Dirty-doc selection + run loop over an open connection (testable core of
/// [`librarian_run`]). Without `force`, only dirty documents are selected:
/// indexed docs whose watermark doesn't match the current content hash and
/// active model (`synth_hash IS NULL OR synth_hash != hash OR synth_model !=
/// ?model`). `--force` selects every document and bypasses the watermark gate.
pub fn librarian_run_on(
    conn: &mut rusqlite::Connection,
    error_log_dir: Option<&std::path::Path>,
    model: &str,
    force: bool,
) -> Result<()> {
    let docs: Vec<(i64, String)> = if force {
        let mut stmt = conn.prepare("SELECT id, path FROM documents ORDER BY path")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, path FROM documents
             WHERE status = 'indexed'
               AND (synth_hash IS NULL OR synth_hash != hash OR synth_model != ?1)
             ORDER BY path",
        )?;
        let rows = stmt.query_map([model], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };

    let scope = if force { "all" } else { "dirty" };
    println!(
        "running librarian over {} {scope} document(s) with fallback model {model}",
        docs.len()
    );

    // Synthesis failures are recorded to <brain>/errors.log by
    // write_synthesis_error (called from generate_summary, which still
    // returns Ok). Surface them by watching that file grow across each
    // call. The log directory must be resolved the same way the writer
    // resolves its target (vault root), not from db_path's parent.
    let error_log = error_log_dir.map(|dir| dir.join("errors.log"));
    let paths: Vec<String> = docs.into_iter().map(|(_, p)| p).collect();

    run_librarian_docs(
        &paths,
        |path| {
            tauri_app_lib::librarian::generate_summary(conn, path, model, force)
                .map_err(|e| format!("{e:#}"))
        },
        &mut std::io::stderr(),
        error_log.as_deref(),
    );
    Ok(())
}

/// doc, commit result). Extracted from `approve_pending_proposals.rs`.
pub fn approve_one(proposal_id: &str) -> Result<()> {
    let paths = retrieval::resolve_brain_paths();
    let mut db = AppDb::open_with_config(&paths.db_path, &paths.config_path)?;
    approve_one_on(&mut db.0, proposal_id)
}

fn approve_one_on(conn: &mut rusqlite::Connection, pid: &str) -> Result<()> {
    let detail =
        get_proposal_detail(conn, pid)?.with_context(|| format!("proposal {pid} not found"))?;
    if detail.status != "pending" {
        bail!("proposal {pid} not pending (status={})", detail.status);
    }
    let decisions: Vec<ItemDecision> = detail
        .items
        .iter()
        .map(|i| ItemDecision {
            item_id: i.id.clone(),
            decision: ItemDecisionKind::Accept,
            edited_payload: None,
        })
        .collect();
    let result = resolve_proposal(
        conn,
        pid,
        &decisions,
        None,
        ResolveOptions { auto_approve: true },
    )?;
    println!(
        "approved {pid}: items={} source={} committed={} conflicts={} dropped_edges={} status={}",
        decisions.len(),
        detail
            .source_doc_paths
            .first()
            .map(String::as_str)
            .unwrap_or("-"),
        result.committed.len(),
        result.conflicts.len(),
        result.dropped_edges.len(),
        result.proposal_status,
    );
    Ok(())
}

/// Approve every pending proposal via [`approve_one_on`]. Continues past
/// individual failures so one bad proposal doesn't block the rest. Prints
/// `approved: N` (N=0 on an empty pending set — still exit 0), or
/// `approved: N, failed: M` before returning Err when any failed.
pub fn approve_all() -> Result<()> {
    let paths = retrieval::resolve_brain_paths();
    let mut db = AppDb::open_with_config(&paths.db_path, &paths.config_path)?;
    let ids: Vec<String> = {
        let mut stmt =
            db.0.prepare("SELECT id FROM curated_proposals WHERE status = 'pending'")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut approved = 0usize;
    let mut failures: Vec<(String, anyhow::Error)> = Vec::new();
    for pid in &ids {
        match approve_one_on(&mut db.0, pid) {
            Ok(()) => approved += 1,
            Err(e) => failures.push((pid.clone(), e)),
        }
    }
    if failures.is_empty() {
        println!("approved: {approved}");
        return Ok(());
    }
    println!("approved: {approved}, failed: {}", failures.len());
    for (pid, e) in &failures {
        eprintln!("failed {pid}: {e:#}");
    }
    bail!(
        "{} of {} proposal(s) failed to approve",
        failures.len(),
        ids.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri_app_lib::db::connection::open_in_memory;
    use tauri_app_lib::db::queries::upsert_document;

    fn seed_doc(conn: &Connection, path: &str, hash: &str, status: &str) -> i64 {
        let id = upsert_document(conn, path, hash).unwrap();
        conn.execute(
            "UPDATE documents SET status = ?2 WHERE id = ?1",
            rusqlite::params![id, status],
        )
        .unwrap();
        id
    }

    fn dirty_paths(conn: &mut Connection, model: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, path FROM documents
                 WHERE status = 'indexed'
                   AND (synth_hash IS NULL OR synth_hash != hash OR synth_model != ?1)
                 ORDER BY path",
            )
            .unwrap();
        let rows = stmt.query_map([model], |r| r.get::<_, String>(1)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    #[test]
    fn dirty_select_returns_only_changed_and_new_docs() {
        let mut conn = open_in_memory().unwrap();
        // New doc: never synthesized (synth_hash NULL).
        let a = seed_doc(&mut conn, "/v/a.md", "hash-a", "indexed");
        // Changed doc: stale watermark hash.
        let b = seed_doc(&mut conn, "/v/b.md", "hash-b", "indexed");
        conn.execute(
            "UPDATE documents SET synth_hash = 'stale', synth_model = 'm' WHERE id = ?1",
            [b.to_string()],
        )
        .unwrap();
        // Clean doc: watermark matches hash + model.
        let c = seed_doc(&mut conn, "/v/c.md", "hash-c", "indexed");
        conn.execute(
            "UPDATE documents SET synth_hash = hash, synth_model = 'm', synth_at = 1 WHERE id = ?1",
            [c.to_string()],
        )
        .unwrap();
        // Model-switched doc: same hash but different synth_model.
        let d = seed_doc(&mut conn, "/v/d.md", "hash-d", "indexed");
        conn.execute(
            "UPDATE documents SET synth_hash = hash, synth_model = 'other-model' WHERE id = ?1",
            [d.to_string()],
        )
        .unwrap();
        // Non-indexed doc must never be selected.
        seed_doc(&mut conn, "/v/e.md", "hash-e", "pending");

        assert_eq!(
            dirty_paths(&mut conn, "m"),
            vec!["/v/a.md", "/v/b.md", "/v/d.md"]
        );
    }
}

/// End-of-run totals for [`run_librarian_docs`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LibrarianRunSummary {
    pub attempted: usize,
    pub ok: usize,
    pub error: usize,
    /// Phase-1 synthesis-watermark gate counter. Dirty-doc selection has
    /// landed, but skipped docs are filtered out of the run loop *before* this
    /// counter is incremented — so this stays 0 in practice today. The field
    /// is reserved for a future phase that counts (rather than drops) them.
    pub skipped_by_watermark: usize,
    pub elapsed_secs: u64,
}

pub(crate) fn format_progress(
    n: usize,
    total: usize,
    path: &str,
    status: &str,
    elapsed_secs: u64,
) -> String {
    format!("[{n}/{total}] {path} {status} ({elapsed_secs}s)")
}

pub(crate) fn format_run_summary(summary: &LibrarianRunSummary) -> String {
    format!(
        "librarian run summary: attempted={} ok={} error={} \
         skipped_by_watermark={} elapsed={}s",
        summary.attempted,
        summary.ok,
        summary.error,
        summary.skipped_by_watermark,
        summary.elapsed_secs
    )
}

fn errors_log_len(path: Option<&Path>) -> u64 {
    path.and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .unwrap_or(0)
}

/// First line of whatever was appended to the errors log at `from` offset.
fn errors_log_tail(path: &Path, from: u64) -> Option<String> {
    use std::io::{Read, Seek};
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(std::io::SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let line = text.lines().next()?.trim().to_string();
    if line.is_empty() {
        None
    } else {
        Some(line)
    }
}

/// Per-doc librarian loop with stderr observability. One `[n/total] <path>
/// ok|error (<elapsed>s)` line per doc plus a final run-summary line, all
/// written (flushed) to `err`. Synthesis failures are surfaced whether they
/// come back as `Err` or were swallowed into `error_log` by
/// `write_synthesis_error` (detected via log growth during the call).
pub(crate) fn run_librarian_docs<F>(
    docs: &[String],
    mut synthesize: F,
    err: &mut dyn std::io::Write,
    error_log: Option<&Path>,
) -> LibrarianRunSummary
where
    F: FnMut(&str) -> std::result::Result<(), String>,
{
    let start = std::time::Instant::now();
    let total = docs.len();
    let mut summary = LibrarianRunSummary {
        attempted: total,
        ..Default::default()
    };

    for (i, path) in docs.iter().enumerate() {
        let log_before = errors_log_len(error_log);
        let doc_start = std::time::Instant::now();
        let result = synthesize(path);
        let elapsed = doc_start.elapsed().as_secs();

        let mut status = "ok";
        match result {
            Ok(()) => {
                if let Some(log) = error_log {
                    // TODO(pr-followup): errors_log_len/tail reads the shared
                    // error log without coordinating with concurrent writers.
                    // If the librarian pipeline is writing errors.log at the
                    // same moment this check runs (e.g. during a parallel
                    // librarian --force + watcher ingest), the > log_before
                    // check can misattribute an unrelated writer's entry to
                    // the doc currently being synthesized. Flagged by
                    // aws-cloud-agent-pr-review on PR #84 as a minor
                    // concurrency concern; not blocking this PR. Filed in
                    // procedures/curated-thoughts-improvement-backlog.md.
                    if errors_log_len(error_log) > log_before {
                        status = "error";
                        let detail = errors_log_tail(log, log_before).unwrap_or_default();
                        let _ = writeln!(
                            err,
                            "error: synthesis failed for {path} — recorded in {}: {detail}",
                            log.display()
                        );
                    }
                }
            }
            Err(e) => {
                status = "error";
                let _ = writeln!(err, "error: synthesis failed for {path}: {e}");
            }
        }
        if status == "ok" {
            summary.ok += 1;
        } else {
            summary.error += 1;
        }

        let _ = writeln!(
            err,
            "{}",
            format_progress(i + 1, total, path, status, elapsed)
        );
        let _ = err.flush();
    }

    summary.elapsed_secs = start.elapsed().as_secs();
    let _ = writeln!(err, "{}", format_run_summary(&summary));
    let _ = err.flush();
    summary
}


        #[cfg(test)]
        mod observability_tests {

        use super::*;
        use std::io::Cursor;

        fn lines(out: &str) -> Vec<&str> {
            out.lines().collect()
        }

        #[test]
        fn progress_line_format_matches_spec() {
            assert_eq!(
                format_progress(3, 10, "docs/a.md", "ok", 12),
                "[3/10] docs/a.md ok (12s)"
            );
            assert_eq!(
                format_progress(4, 10, "docs/b.md", "error", 1),
                "[4/10] docs/b.md error (1s)"
            );
        }

        #[test]
        fn summary_format_includes_reserved_watermark_field() {
            let s = LibrarianRunSummary {
                attempted: 5,
                ok: 4,
                error: 1,
                skipped_by_watermark: 0,
                elapsed_secs: 42,
            };
            assert_eq!(
                format_run_summary(&s),
                "librarian run summary: attempted=5 ok=4 error=1 skipped_by_watermark=0 elapsed=42s"
            );
        }

        #[test]
        fn per_doc_lines_and_counts_all_ok() {
            let docs = vec!["a.md".to_string(), "b.md".to_string()];
            let mut out = Cursor::new(Vec::new());
            let summary = run_librarian_docs(&docs, |_p| Ok(()), &mut out, None);
            let text = String::from_utf8(out.into_inner()).unwrap();
            let ls = lines(&text);
            assert_eq!(ls[0], "[1/2] a.md ok (0s)");
            assert_eq!(ls[1], "[2/2] b.md ok (0s)");
            assert_eq!(
                ls[2],
                "librarian run summary: attempted=2 ok=2 error=0 skipped_by_watermark=0 elapsed=0s"
            );
            assert_eq!(
                summary,
                LibrarianRunSummary {
                    attempted: 2,
                    ok: 2,
                    error: 0,
                    skipped_by_watermark: 0,
                    elapsed_secs: 0
                }
            );
        }

        #[test]
        fn err_result_counted_and_surfaced() {
            let docs = vec!["ok.md".to_string(), "bad.md".to_string()];
            let mut out = Cursor::new(Vec::new());
            let summary = run_librarian_docs(
                &docs,
                |p| {
                    if p == "bad.md" {
                        Err("LLM unreachable".to_string())
                    } else {
                        Ok(())
                    }
                },
                &mut out,
                None,
            );
            let text = String::from_utf8(out.into_inner()).unwrap();
            assert!(text.contains("error: synthesis failed for bad.md: LLM unreachable"));
            assert!(text.contains("[2/2] bad.md error ("));
            assert_eq!(summary.ok, 1);
            assert_eq!(summary.error, 1);
            assert_eq!(summary.attempted, 2);
        }

        #[test]
        fn swallowed_synthesis_error_via_log_growth_is_surfaced() {
            let dir = tempfile::TempDir::new().unwrap();
            let log = dir.path().join("errors.log");
            std::fs::write(&log, "pre-existing\n").unwrap();

            let docs = vec!["swallow.md".to_string(), "fine.md".to_string()];
            let log_path = log.clone();
            let mut out = Cursor::new(Vec::new());
            let summary = run_librarian_docs(
                &docs,
                move |p| {
                    if p == "swallow.md" {
                        // Mirrors write_synthesis_error: append + return Ok.
                        use std::io::Write as _;
                        let mut f = std::fs::OpenOptions::new()
                            .append(true)
                            .open(&log_path)
                            .unwrap();
                        writeln!(
                            f,
                            "[1756123456] synthesis JSON failure for swallow.md: boom"
                        )
                        .unwrap();
                    }
                    Ok(())
                },
                &mut out,
                Some(&log),
            );
            let text = String::from_utf8(out.into_inner()).unwrap();
            assert!(text.contains("recorded in"));
            assert!(text.contains("errors.log"));
            assert!(text.contains("synthesis JSON failure for swallow.md"));
            assert!(text.contains("[1/2] swallow.md error ("));
            assert!(text.contains("[2/2] fine.md ok ("));
            assert_eq!(summary.error, 1);
            assert_eq!(summary.ok, 1);
        }

        #[test]
        fn empty_run_still_prints_summary() {
            let mut out = Cursor::new(Vec::new());
            let summary = run_librarian_docs(&[], |_p| Ok(()), &mut out, None);
            let text = String::from_utf8(out.into_inner()).unwrap();
            assert_eq!(
                text.trim(),
                "librarian run summary: attempted=0 ok=0 error=0 skipped_by_watermark=0 elapsed=0s"
            );
            assert_eq!(summary.attempted, 0);
        }
    }

