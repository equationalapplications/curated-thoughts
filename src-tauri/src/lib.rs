pub mod chunker;
pub mod db;
pub mod embedder;
mod hasher;
pub mod librarian;
mod pipeline;
pub mod search;
pub mod retrieval;
pub mod scifact_fixture;
pub mod recall_bench_fixture;
mod setup;
pub mod vault;
mod watcher;

use std::sync::{mpsc::SyncSender, Mutex};
use tauri::{AppHandle, Emitter, State};
use db::AppDb;
use chunker::should_ingest_extension;
use pipeline::start_pipeline;
#[cfg(not(feature = "test-utils"))]
use pipeline::PipelineJob;
use rusqlite::types::Value as SqlVal;
use serde_json::Value as JsonVal;
use setup::{check_ollama as ollama_check, list_local_models as ollama_models,
            pull_model as ollama_pull, recommended_model as ollama_recommended,
            start_ollama_server as ollama_start, OllamaStatus};
use vault::VaultConfig;
use watcher::{start_watcher, VaultEvent};

struct DbState(Mutex<AppDb>);
struct VaultConfigState(Mutex<VaultConfig>);
struct PipelineTx(Mutex<SyncSender<PipelineJob>>);

// ── Vault commands ────────────────────────────────────────────────────────────

#[tauri::command]
fn get_vault_path(state: State<VaultConfigState>) -> Result<Option<String>, String> {
    state.0.lock().unwrap().get_vault_path().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_vault_path(path: String, state: State<VaultConfigState>) -> Result<(), String> {
    // trusted: vault root from Tauri file picker dialog (user selects directory)
    state.0.lock().unwrap().set_vault_path(&path).map_err(|e| e.to_string())?;
    let root = std::path::Path::new(&path);
    for subdir in &["documents", "wiki"] {
        std::fs::create_dir_all(root.join(subdir)).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(root.join(".brain").join("converted")).map_err(|e| e.to_string())?;
    Ok(())
}

// ── Watcher + pipeline ────────────────────────────────────────────────────────

#[tauri::command]
fn start_file_watcher(
    app: AppHandle,
    pipeline: State<PipelineTx>,
    db_state: State<DbState>,
    vault_state: State<VaultConfigState>,
) -> Result<(), String> {
    // Get configured vault root from state (trusted source)
    let configured_vault = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault configured".to_string())?;
    let vault_root = std::path::PathBuf::from(&configured_vault);

    // Canonicalize vault root once
    let vault_canonical = vault_root
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize configured vault: {}", e))?;

    let tx = pipeline.0.lock().unwrap().clone();

    let raw_docs = vault_canonical.join("documents");
    // Canonicalize so macOS FSEvents paths (which are real paths) match correctly.
    let documents_root = std::fs::canonicalize(&raw_docs).unwrap_or(raw_docs.clone());

    // ── Startup reconciliation ────────────────────────────────────────────────
    // Purge DB entries whose files no longer exist on disk, and queue ingest for
    // files on disk that aren't yet indexed. Handles changes made while the app
    // was closed and any path-format drift.
    {
        let guard = db_state.0.lock().unwrap();
        let conn = &guard.0;

        // 1. DB paths that no longer exist on disk → delete
        let db_paths: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT path FROM documents WHERE tier = 'user_doc'")
                .map_err(|e| e.to_string())?;
            let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
            let mut v = Vec::new();
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                v.push(row.get::<_, String>(0).map_err(|e| e.to_string())?);
            }
            v
        };
        for path in db_paths {
            if !std::path::Path::new(&path).exists() {
                eprintln!("[reconcile] purging deleted file from index: {}", path);
                let _ = tx.try_send(PipelineJob::Delete(path));
            }
        }

        // 2. Files on disk → queue ingest (pipeline skips if hash unchanged)
        if raw_docs.exists() {
            for entry in walkdir::WalkDir::new(&raw_docs)
                .min_depth(1)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file())
            {
                let ext = entry.path().extension().and_then(|s| s.to_str()).unwrap_or("");
                if should_ingest_extension(ext) {
                    let normalized = std::fs::canonicalize(entry.path())
                        .unwrap_or_else(|_| entry.path().to_path_buf())
                        .to_string_lossy()
                        .into_owned();
                    let _ = tx.try_send(PipelineJob::ingest(
                        normalized,
                    ));
                }
            }
        }
    }

    start_watcher(vault_canonical, move |event| {
        let _ = app.emit("vault-event", &event);
        let path_str = match &event {
            VaultEvent::Added(p) | VaultEvent::Modified(p) | VaultEvent::Deleted(p) => p,
        };
        // For existing files, canonicalize to match documents_root.
        // For deleted files (don't exist), fall back to the raw path.
        let canonical = std::fs::canonicalize(path_str)
            .unwrap_or_else(|_| std::path::PathBuf::from(path_str));
        if !canonical.starts_with(&documents_root) {
            return;
        }
        let normalized = canonical.to_string_lossy().into_owned();
        let job = match &event {
            VaultEvent::Added(_) | VaultEvent::Modified(_) => {
                Some(PipelineJob::ingest(normalized.clone()))
            }
            VaultEvent::Deleted(_) => Some(PipelineJob::Delete(normalized.clone())),
        };
        if let Some(j) = job {
            let _ = tx.try_send(j);
        }
    })
    .map_err(|e| e.to_string())
}

// ── Indexing status ───────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct IndexingStatus {
    pub indexed: i64,
    pub pending: i64,
}

#[tauri::command]
fn get_indexing_status(db_state: State<DbState>) -> Result<IndexingStatus, String> {
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let indexed = db::count_indexed_documents(conn).map_err(|e| e.to_string())?;
    let pending = db::count_pending_documents(conn).map_err(|e| e.to_string())?;
    Ok(IndexingStatus { indexed, pending })
}

/// Enqueue ingestion for every indexed user document. After chunk strategy (`ast_*`) or
/// embedding model changes, pass `force_rechunk: true` so work runs even when bytes are
/// unchanged (`Ingest` alone would no-op on matching hash).
#[tauri::command]
fn queue_full_reindex(
    force_rechunk: bool,
    pipeline: State<PipelineTx>,
    db_state: State<DbState>,
) -> Result<usize, String> {
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let paths =
        crate::db::list_indexed_user_doc_paths(conn).map_err(|e| e.to_string())?;
    let tx = pipeline.0.lock().unwrap();
    let mut queued = 0usize;
    for path in paths {
        if !std::path::Path::new(&path).exists() {
            eprintln!("[queue_full_reindex] skip missing file: {path}");
            continue;
        }
        let job = if force_rechunk {
            PipelineJob::rechunk(path)
        } else {
            PipelineJob::ingest(path)
        };
        tx.send(job).map_err(|e| format!("pipeline channel closed: {e}"))?;
        queued += 1;
    }
    Ok(queued)
}

// ── Wiki SQL bridge ───────────────────────────────────────────────────────────
// Implements SQLiteAdapter interface for @equationalapplications/react-llm-wiki

fn json_to_sql(v: &JsonVal) -> SqlVal {
    match v {
        JsonVal::Null => SqlVal::Null,
        JsonVal::Bool(b) => SqlVal::Integer(if *b { 1 } else { 0 }),
        JsonVal::Number(n) => {
            if let Some(i) = n.as_i64() { SqlVal::Integer(i) }
            else { SqlVal::Real(n.as_f64().unwrap_or(0.0)) }
        }
        JsonVal::String(s) => SqlVal::Text(s.clone()),
        JsonVal::Array(a) => SqlVal::Blob(
            a.iter().filter_map(|v| v.as_u64().map(|n| n as u8)).collect()
        ),
        JsonVal::Object(_) => SqlVal::Null,
    }
}

fn row_to_json(
    row: &rusqlite::Row,
    col_names: &[String],
) -> rusqlite::Result<serde_json::Map<String, JsonVal>> {
    let mut map = serde_json::Map::new();
    for (i, name) in col_names.iter().enumerate() {
        let val = match row.get_ref(i)? {
            rusqlite::types::ValueRef::Null => JsonVal::Null,
            rusqlite::types::ValueRef::Integer(n) => JsonVal::Number(n.into()),
            rusqlite::types::ValueRef::Real(f) => {
                serde_json::Number::from_f64(f).map(JsonVal::Number).unwrap_or(JsonVal::Null)
            }
            rusqlite::types::ValueRef::Text(s) => JsonVal::String(String::from_utf8_lossy(s).into()),
            rusqlite::types::ValueRef::Blob(b) => {
                JsonVal::Array(b.iter().map(|&n| JsonVal::Number(n.into())).collect())
            }
        };
        map.insert(name.clone(), val);
    }
    Ok(map)
}

fn query_rows(
    sql: &str,
    params: &[JsonVal],
    conn: &rusqlite::Connection,
) -> Result<Vec<serde_json::Map<String, JsonVal>>, String> {
    let sql_params: Vec<SqlVal> = params.iter().map(json_to_sql).collect();
    let refs: Vec<&dyn rusqlite::ToSql> = sql_params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
    let mut out = Vec::new();
    {
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let mut rows = stmt.query(refs.as_slice()).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            out.push(row_to_json(row, &col_names).map_err(|e| e.to_string())?);
        }
    }
    Ok(out)
}

#[tauri::command]
fn wiki_exec(sql: String, db_state: State<DbState>) -> Result<(), String> {
    db_state.0.lock().unwrap().0.execute_batch(&sql).map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct WikiRunResult { changes: i64, last_insert_row_id: i64 }

#[tauri::command]
fn wiki_run(sql: String, params: Vec<JsonVal>, db_state: State<DbState>) -> Result<WikiRunResult, String> {
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let sql_params: Vec<SqlVal> = params.iter().map(json_to_sql).collect();
    let refs: Vec<&dyn rusqlite::ToSql> = sql_params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
    let changes = conn.execute(&sql, refs.as_slice()).map_err(|e| e.to_string())?;
    Ok(WikiRunResult { changes: changes as i64, last_insert_row_id: conn.last_insert_rowid() })
}

#[tauri::command]
fn wiki_get_all(sql: String, params: Vec<JsonVal>, db_state: State<DbState>) -> Result<Vec<serde_json::Map<String, JsonVal>>, String> {
    query_rows(&sql, &params, &db_state.0.lock().unwrap().0)
}

#[tauri::command]
fn wiki_get_first(sql: String, params: Vec<JsonVal>, db_state: State<DbState>) -> Result<Option<serde_json::Map<String, JsonVal>>, String> {
    Ok(query_rows(&sql, &params, &db_state.0.lock().unwrap().0)?.into_iter().next())
}

// ── Embed text (for wiki llmProvider.embed) ───────────────────────────────────

#[tauri::command]
fn embed_text(text: String, cfg: State<VaultConfigState>) -> Result<Vec<f32>, String> {
    let profile = cfg
        .0
        .lock()
        .unwrap()
        .get_embed_profile()
        .map_err(|e| e.to_string())?;
    crate::embedder::embed_one(&profile, text).map_err(|e| e.to_string())
}

// ── Ollama generate (for wiki llmProvider.generateText) ───────────────────────

#[tauri::command]
async fn ollama_generate(system_prompt: String, user_prompt: String) -> Result<String, String> {
    let model = ollama_recommended();
    let client = reqwest::Client::new();
    let resp = client
        .post("http://localhost:11434/api/generate")
        .json(&serde_json::json!({
            "model": model,
            "system": system_prompt,
            "prompt": user_prompt,
            "stream": false
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let body: JsonVal = resp.json().await.map_err(|e| e.to_string())?;
    body["response"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "missing 'response' in Ollama reply".to_string())
}

// ── Ollama commands ───────────────────────────────────────────────────────────

#[tauri::command]
fn check_ollama() -> OllamaStatus { ollama_check() }

#[tauri::command]
fn list_local_models() -> Result<Vec<String>, String> {
    ollama_models().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_recommended_model() -> String { ollama_recommended().to_string() }

#[tauri::command]
fn start_ollama_server() -> Result<(), String> {
    ollama_start().map_err(|e| e.to_string())
}

#[tauri::command]
fn pull_model(model_id: String, app: AppHandle) -> Result<(), String> {
    ollama_pull(&model_id, move |completed, total| {
        let _ = app.emit(
            "ollama-pull-progress",
            serde_json::json!({ "completed": completed, "total": total }),
        );
    })
    .map_err(|e| e.to_string())
}

// ── Search commands ───────────────────────────────────────────────────────────

#[tauri::command]
fn search_vault(
    query: String,
    limit: usize,
    db_state: State<DbState>,
    cfg: State<VaultConfigState>,
) -> Result<Vec<search::SearchResult>, String> {
    let profile = cfg
        .0
        .lock()
        .unwrap()
        .get_embed_profile()
        .map_err(|e| e.to_string())?;
    let guard = db_state.0.lock().unwrap();
    retrieval::semantic_search_chunks(&guard.0, &profile, &query, limit).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_related_chunks(
    doc_path: String,
    limit: usize,
    db_state: State<DbState>,
    vault_state: State<VaultConfigState>,
) -> Result<Vec<search::SearchResult>, String> {
    // DB stores absolute paths (from watcher), but frontend may send relative paths
    // (from list_vault_files). Normalize to absolute for DB query.
    let normalized_path = {
        let path = std::path::Path::new(&doc_path);
        if path.is_absolute() {
            // Already absolute - canonicalize to match DB format
            path.canonicalize()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or(doc_path)
        } else {
            // Convert relative to absolute, then canonicalize to match DB format
            let root = vault_state
                .0
                .lock()
                .unwrap()
                .get_vault_path()
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "no vault path set".to_string())?;
            let vault_root = std::path::PathBuf::from(&root);
            let joined = vault_root.join(path);
            joined
                .canonicalize()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| joined.to_string_lossy().to_string())
        }
    };
    let guard = db_state.0.lock().unwrap();
    retrieval::related_chunks_facade(&guard.0, &normalized_path, limit).map_err(|e| e.to_string())
}

// ── Vault file listing ────────────────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
pub struct VaultFile {
    pub path: String,
    pub name: String,
    pub tier: String, // "user_doc" | "wiki"
}

#[tauri::command]
fn list_vault_files(state: State<VaultConfigState>) -> Result<Vec<VaultFile>, String> {
    let root = match state.0.lock().unwrap().get_vault_path().map_err(|e| e.to_string())? {
        Some(p) => std::path::PathBuf::from(p),
        None => return Ok(vec![]),
    };

    let mut files = Vec::new();

    for (subdir, tier) in &[("documents", "user_doc"), ("wiki", "wiki")] {
        let dir = root.join(subdir);
        if !dir.exists() { continue; }
        let walker = walkdir::WalkDir::new(&dir)
            .min_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| {
                let ext = e.path().extension().and_then(|s| s.to_str()).unwrap_or("");
                should_ingest_extension(ext)
            });

        for entry in walker {
            // Return vault-relative paths for safe_vault_path compatibility.
            // Skip any entry that can't be made relative (shouldn't happen in normal operation,
            // but could occur if the vault contains symlinks pointing outside).
            let Some(relative) = entry.path().strip_prefix(&root).ok() else {
                continue;
            };
            let path = relative.to_string_lossy().to_string();
            let name = entry.file_name().to_string_lossy().to_string();
            files.push(VaultFile { path, name, tier: tier.to_string() });
        }
    }

    Ok(files)
}

#[tauri::command]
fn read_document(path: String, state: State<VaultConfigState>) -> Result<String, String> {
    let root = match state.0.lock().unwrap().get_vault_path().map_err(|e| e.to_string())? {
        Some(p) => std::path::PathBuf::from(p),
        None => return Err("no vault path set".to_string()),
    };

    // Normalize path: if absolute and starts with vault root, make it vault-relative.
    // Preserves backward compatibility with DB/search results (which store absolute paths)
    // while still enforcing containment via safe_vault_path.
    let normalized_path = {
        let candidate = std::path::Path::new(&path);
        if candidate.is_absolute() {
            // Try canonical normalization for robust symlink/casing handling
            match (candidate.canonicalize(), root.canonicalize()) {
                (Ok(can_candidate), Ok(can_root)) => {
                    can_candidate
                        .strip_prefix(&can_root)
                        .map(|rel| rel.to_string_lossy().to_string())
                        .unwrap_or(path.clone())
                }
                _ => {
                    // Fall back to non-canonical strip_prefix
                    candidate
                        .strip_prefix(&root)
                        .map(|rel| rel.to_string_lossy().to_string())
                        .unwrap_or(path.clone())
                }
            }
        } else {
            path.clone()
        }
    };

    let safe = crate::vault::safe_vault_path(
        &root,
        &normalized_path,
        &["documents", "wiki"],
        crate::vault::PathMode::MustExist,
    )
    .map_err(|e| e.to_string())?;

    std::fs::read_to_string(&safe).map_err(|e| e.to_string())
}

// ── Review queue ──────────────────────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
pub struct ReviewPage {
    pub id: i64,
    pub path: String,
    pub source_doc_ids: String,
    pub generated_by: String,
}

#[tauri::command]
fn get_review_queue(db_state: State<DbState>) -> Result<Vec<ReviewPage>, String> {
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let mut stmt = conn
        .prepare(
            "SELECT id, path, source_doc_ids, generated_by
             FROM wiki_pages WHERE status = 'pending_review'
             ORDER BY id DESC",
        )
        .map_err(|e| e.to_string())?;
    let mut pages = Vec::new();
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        pages.push(ReviewPage {
            id: row.get(0).map_err(|e| e.to_string())?,
            path: row.get(1).map_err(|e| e.to_string())?,
            source_doc_ids: row.get(2).map_err(|e| e.to_string())?,
            generated_by: row.get(3).map_err(|e| e.to_string())?,
        });
    }
    Ok(pages)
}

#[tauri::command]
fn approve_wiki_page(
    id: i64,
    content: String,
    db_state: State<DbState>,
    vault_state: State<VaultConfigState>,
) -> Result<(), String> {
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let page_path: String = conn
        .query_row("SELECT path FROM wiki_pages WHERE id = ?1", [id], |r| r.get(0))
        .map_err(|e| e.to_string())?;

    let vault_path = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault configured".to_string())?;
    let vault_root = std::path::PathBuf::from(&vault_path);
    std::fs::create_dir_all(vault_root.join("wiki")).map_err(|e| e.to_string())?;

    // Reject absolute paths before normalization
    if std::path::Path::new(&page_path).is_absolute() {
        return Err("absolute paths not allowed".to_string());
    }

    // Normalize path: if it doesn't start with "wiki/", prepend it for backward compatibility
    let normalized_path = if page_path.starts_with("wiki/") {
        page_path.clone()
    } else {
        format!("wiki/{}", page_path)
    };

    let safe = crate::vault::safe_vault_path(
        &vault_root,
        &normalized_path,
        &["wiki"],
        crate::vault::PathMode::MayCreate,
    )
    .map_err(|e| e.to_string())?;

    std::fs::write(&safe, &content).map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE wiki_pages SET status = 'approved', last_synced = unixepoch() WHERE id = ?1",
        [id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn reject_wiki_page(id: i64, db_state: State<DbState>) -> Result<(), String> {
    db_state
        .0
        .lock()
        .unwrap()
        .0
        .execute("UPDATE wiki_pages SET status = 'rejected' WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ── Folder rules ──────────────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct FolderRule {
    pub id: i64,
    pub folder_path: String,
    pub librarian_mode: String,
    pub auto_approve: bool,
}

#[tauri::command]
fn get_folder_rules(db_state: State<DbState>) -> Result<Vec<FolderRule>, String> {
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let mut stmt = conn
        .prepare("SELECT id, folder_path, librarian_mode, auto_approve FROM folder_rules ORDER BY folder_path")
        .map_err(|e| e.to_string())?;
    let mut rules = Vec::new();
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        rules.push(FolderRule {
            id: row.get(0).map_err(|e| e.to_string())?,
            folder_path: row.get(1).map_err(|e| e.to_string())?,
            librarian_mode: row.get(2).map_err(|e| e.to_string())?,
            auto_approve: row.get::<_, i64>(3).map_err(|e| e.to_string())? != 0,
        });
    }
    Ok(rules)
}

#[tauri::command]
fn set_folder_rule(
    folder_path: String,
    librarian_mode: String,
    auto_approve: bool,
    db_state: State<DbState>,
) -> Result<(), String> {
    let auto_i: i64 = if auto_approve { 1 } else { 0 };
    db_state
        .0
        .lock()
        .unwrap()
        .0
        .execute(
            "INSERT INTO folder_rules (folder_path, librarian_mode, auto_approve)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(folder_path) DO UPDATE SET librarian_mode = ?2, auto_approve = ?3",
            rusqlite::params![folder_path, librarian_mode, auto_i],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn delete_folder_rule(id: i64, db_state: State<DbState>) -> Result<(), String> {
    db_state
        .0
        .lock()
        .unwrap()
        .0
        .execute("DELETE FROM folder_rules WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn get_proposed_content(
    page_id: i64,
    db_state: State<DbState>,
    vault_state: State<VaultConfigState>,
) -> Result<String, String> {
    let page_path: String = {
        let guard = db_state.0.lock().unwrap();
        guard.0
            .query_row("SELECT path FROM wiki_pages WHERE id = ?1", [page_id], |r| r.get(0))
            .map_err(|e| e.to_string())?
    };
    let vault = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault set".to_string())?;
    let vault_root = std::path::PathBuf::from(&vault);

    let safe = crate::vault::safe_vault_path(
        &vault_root,
        &format!(".brain/proposed/{}", page_path),
        &[".brain/proposed"],
        crate::vault::PathMode::MustExist,
    );

    Ok(match safe {
        Ok(p) => std::fs::read_to_string(&p)
            .unwrap_or_else(|_| format!("# {}\n\n*Proposed wiki page — content not available.*", page_path)),
        Err(_) => format!("# {}\n\n*Proposed wiki page — content not available.*", page_path),
    })
}

#[tauri::command]
fn save_wiki_page(
    path: String,
    content: String,
    vault_state: State<VaultConfigState>,
) -> Result<(), String> {
    let vault = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or("no vault set".to_string())?;
    let vault_root = std::path::PathBuf::from(&vault);
    // Ensure the allowed subdir exists before resolving the user path.
    std::fs::create_dir_all(vault_root.join("wiki")).map_err(|e| e.to_string())?;

    // Reject absolute paths before normalization
    if std::path::Path::new(&path).is_absolute() {
        return Err("absolute paths not allowed".to_string());
    }

    // Normalize path: if it doesn't start with "wiki/", prepend it for backward compatibility
    let normalized_path = if path.starts_with("wiki/") {
        path.clone()
    } else {
        format!("wiki/{}", path)
    };

    let safe = crate::vault::safe_vault_path(
        &vault_root,
        &normalized_path,
        &["wiki"],
        crate::vault::PathMode::MayCreate,
    )
    .map_err(|e| e.to_string())?;

    std::fs::write(&safe, &content).map_err(|e| e.to_string())?;
    Ok(())
}

// ── File management ───────────────────────────────────────────────────────────

#[tauri::command]
fn delete_vault_file(path: String, state: State<VaultConfigState>) -> Result<(), String> {
    let root = state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault set".to_string())?;
    let vault_root = std::path::PathBuf::from(&root);

    // Normalize path: if absolute and starts with vault root, make it vault-relative.
    // Preserves backward compatibility with DB/search results (which store absolute paths)
    // while still enforcing containment via safe_vault_path.
    let normalized_path = {
        let candidate = std::path::Path::new(&path);
        if candidate.is_absolute() {
            // Try canonical normalization for robust symlink/casing handling
            match (candidate.canonicalize(), vault_root.canonicalize()) {
                (Ok(can_candidate), Ok(can_root)) => {
                    can_candidate
                        .strip_prefix(&can_root)
                        .map(|rel| rel.to_string_lossy().to_string())
                        .unwrap_or(path.clone())
                }
                _ => {
                    // Fall back to non-canonical strip_prefix
                    candidate
                        .strip_prefix(&vault_root)
                        .map(|rel| rel.to_string_lossy().to_string())
                        .unwrap_or(path.clone())
                }
            }
        } else {
            path.clone()
        }
    };

    let safe = crate::vault::safe_vault_path(
        &vault_root,
        &normalized_path,
        &["documents"],
        crate::vault::PathMode::MustExist,
    )
    .map_err(|e| e.to_string())?;

    std::fs::remove_file(&safe).map_err(|e| e.to_string())
}

#[tauri::command]
fn copy_to_vault(src_path: String, vault_state: State<VaultConfigState>) -> Result<String, String> {
    let src = std::path::Path::new(&src_path);
    let file_name = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "invalid filename".to_string())?;

    let vault_path = vault_state
        .0
        .lock()
        .unwrap()
        .get_vault_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no vault configured".to_string())?;
    let vault_root = std::path::PathBuf::from(&vault_path);
    std::fs::create_dir_all(vault_root.join("documents")).map_err(|e| e.to_string())?;

    let dest = crate::vault::safe_vault_path(
        &vault_root,
        &format!("documents/{}", file_name),
        &["documents"],
        crate::vault::PathMode::MayCreate,
    )
    .map_err(|e| e.to_string())?;

    std::fs::copy(src, &dest).map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().into_owned())
}

// ── Test utilities ────────────────────────────────────────────────────────────

pub use pipeline::ingest_document;

#[cfg(feature = "test-utils")]
pub use pipeline::{PipelineJob, PipelineWorker};

#[cfg(feature = "test-utils")]
pub fn make_test_app(tmp_path: &std::path::Path) -> tauri::App<tauri::test::MockRuntime> {
    let db = db::AppDb::open(&tmp_path.join("brain.db")).expect("open test db");
    let config = vault::VaultConfig::new(tmp_path.join("config.json"));
    let (tx, _rx) = std::sync::mpsc::sync_channel::<PipelineJob>(1);
    tauri::test::mock_builder()
        .manage(DbState(std::sync::Mutex::new(db)))
        .manage(VaultConfigState(std::sync::Mutex::new(config)))
        .manage(PipelineTx(std::sync::Mutex::new(tx)))
        .invoke_handler(tauri::generate_handler![
            get_vault_path,
            set_vault_path,
            get_review_queue,
            approve_wiki_page,
            reject_wiki_page,
            get_proposed_content,
            get_folder_rules,
            set_folder_rule,
            delete_folder_rule,
            get_indexing_status,
            list_vault_files,
            read_document,
            search_vault,
            get_related_chunks,
            save_wiki_page,
            queue_full_reindex,
        ])
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap()
}

// ── App entry ─────────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let brain_dir = dirs::home_dir().unwrap_or_default().join(".brain");
    std::fs::create_dir_all(&brain_dir).ok();

    let db_path = brain_dir.join("brain.db");
    let db = AppDb::open(&db_path).expect("failed to open database");
    let config = VaultConfig::new(brain_dir.join("config.json"));
    let pipeline_tx = start_pipeline(db_path);

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .manage(DbState(Mutex::new(db)))
        .manage(VaultConfigState(Mutex::new(config)))
        .manage(PipelineTx(Mutex::new(pipeline_tx)))
        .invoke_handler(tauri::generate_handler![
            get_vault_path,
            set_vault_path,
            start_file_watcher,
            get_indexing_status,
            queue_full_reindex,
            wiki_exec,
            wiki_run,
            wiki_get_all,
            wiki_get_first,
            embed_text,
            ollama_generate,
            check_ollama,
            list_local_models,
            pull_model,
            start_ollama_server,
            get_recommended_model,
            search_vault,
            get_related_chunks,
            list_vault_files,
            read_document,
            get_review_queue,
            approve_wiki_page,
            reject_wiki_page,
            get_folder_rules,
            set_folder_rule,
            delete_folder_rule,
            get_proposed_content,
            save_wiki_page,
            copy_to_vault,
            delete_vault_file,
        ])
        .run(tauri::generate_context!())
        .expect("error running Tauri application");
}
