mod chunker;
mod db;
mod embedder;
mod hasher;
mod librarian;
mod pipeline;
mod search;
mod setup;
mod vault;
mod watcher;

use std::sync::{mpsc::SyncSender, Mutex};
use tauri::{AppHandle, Emitter, State};
use db::AppDb;
use embedder::Embedder;
use pipeline::{start_pipeline, PipelineJob};
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
struct WikiEmbedder(Mutex<Option<Embedder>>);

// ── Vault commands ────────────────────────────────────────────────────────────

#[tauri::command]
fn get_vault_path(state: State<VaultConfigState>) -> Result<Option<String>, String> {
    state.0.lock().unwrap().get_vault_path().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_vault_path(path: String, state: State<VaultConfigState>) -> Result<(), String> {
    state.0.lock().unwrap().set_vault_path(&path).map_err(|e| e.to_string())?;
    let root = std::path::Path::new(&path);
    for subdir in &["documents", "wiki"] {
        std::fs::create_dir_all(root.join(subdir)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ── Watcher + pipeline ────────────────────────────────────────────────────────

#[tauri::command]
fn start_file_watcher(
    vault_path: String,
    app: AppHandle,
    pipeline: State<PipelineTx>,
) -> Result<(), String> {
    let tx = pipeline.0.lock().unwrap().clone();
    let documents_root = std::path::PathBuf::from(&vault_path).join("documents");
    start_watcher(vault_path.into(), move |event| {
        let _ = app.emit("vault-event", &event);
        let path_str = match &event {
            VaultEvent::Added(p) | VaultEvent::Modified(p) | VaultEvent::Deleted(p) => p,
        };
        if !std::path::Path::new(path_str).starts_with(&documents_root) {
            return;
        }
        let job = match &event {
            VaultEvent::Added(p) | VaultEvent::Modified(p) => Some(PipelineJob::Ingest(p.clone())),
            VaultEvent::Deleted(p) => Some(PipelineJob::Delete(p.clone())),
        };
        if let Some(j) = job { let _ = tx.try_send(j); }
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
fn embed_text(text: String, embedder_state: State<WikiEmbedder>) -> Result<Vec<f32>, String> {
    let mut guard = embedder_state.0.lock().unwrap();
    if guard.is_none() {
        *guard = Some(Embedder::new().map_err(|e| e.to_string())?);
    }
    guard.as_ref().unwrap()
        .embed(vec![text])
        .map_err(|e| e.to_string())
        .map(|mut vecs| vecs.drain(..).next().unwrap_or_default())
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
    embedder_state: State<WikiEmbedder>,
) -> Result<Vec<search::SearchResult>, String> {
    let query_vec = {
        let mut guard = embedder_state.0.lock().unwrap();
        if guard.is_none() {
            *guard = Some(Embedder::new().map_err(|e| e.to_string())?);
        }
        guard
            .as_ref()
            .unwrap()
            .embed(vec![query])
            .map_err(|e| e.to_string())?
            .into_iter()
            .next()
            .unwrap_or_default()
    };
    let guard = db_state.0.lock().unwrap();
    search::semantic_search(&guard.0, &query_vec, limit.clamp(1, 50))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_related_chunks(
    doc_path: String,
    limit: usize,
    db_state: State<DbState>,
) -> Result<Vec<search::SearchResult>, String> {
    let guard = db_state.0.lock().unwrap();
    search::related_chunks(&guard.0, &doc_path, limit.clamp(1, 10))
        .map_err(|e| e.to_string())
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
                matches!(ext, "md" | "txt" | "markdown" | "pdf" | "docx")
            });

        for entry in walker {
            let path = entry.path().to_string_lossy().to_string();
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

    let doc_path = std::path::Path::new(&path);

    if !doc_path.starts_with(&root) {
        return Err("path outside vault".to_string());
    }
    let in_documents = doc_path.starts_with(root.join("documents"));
    let in_wiki = doc_path.starts_with(root.join("wiki"));
    if !in_documents && !in_wiki {
        return Err("path not in documents/ or wiki/".to_string());
    }

    std::fs::read_to_string(doc_path).map_err(|e| e.to_string())
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
    vault_path: String,
    db_state: State<DbState>,
) -> Result<(), String> {
    let guard = db_state.0.lock().unwrap();
    let conn = &guard.0;
    let page_path: String = conn
        .query_row("SELECT path FROM wiki_pages WHERE id = ?1", [id], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let wiki_dir = std::path::Path::new(&vault_path).join("wiki");
    std::fs::create_dir_all(&wiki_dir).map_err(|e| e.to_string())?;
    std::fs::write(wiki_dir.join(&page_path), &content).map_err(|e| e.to_string())?;
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
        .manage(WikiEmbedder(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            get_vault_path,
            set_vault_path,
            start_file_watcher,
            get_indexing_status,
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
        ])
        .run(tauri::generate_context!())
        .expect("error running Tauri application");
}
