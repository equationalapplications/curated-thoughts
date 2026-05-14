use anyhow::Result;
use rusqlite::Connection;
use std::{
    collections::HashSet,
    io::Read,
    path::{Path, PathBuf},
    sync::{mpsc, Arc, atomic::{AtomicUsize, Ordering}},
};

use crate::chunker::{chunk_autodetect, AstLang, ChunkStrategy, should_ingest_extension};
use crate::indexer::{extract_references, RefLang};
use crate::db::queries::{
    delete_document, delete_document_chunks, get_document_by_path, insert_chunk, insert_embedding,
    mark_document_error, mark_document_indexed, upsert_document,
};
use crate::embedder::{embed_batch, EmbedProfile};
use crate::hasher::hash_bytes;
use crate::vault::VaultConfig;

#[derive(Debug, Clone)]
pub enum PipelineJob {
    /// Chunk + embed path. With `force: true`, re-runs chunking even when content hash unchanged
    /// (chunk strategy upgrades, embedding model swaps).
    /// `count_pending: true` means this job contributes to the active ingesting counter and the
    /// worker must decrement it on completion.
    Ingest {
        path: String,
        force: bool,
        count_pending: bool,
    },
    Delete(String),
}

pub enum PipelineStatusEvent {
    PendingCount(usize),
}

impl PipelineJob {
    pub fn ingest(path: impl Into<String>) -> Self {
        Self::Ingest {
            path: path.into(),
            force: false,
            count_pending: false,
        }
    }

    pub fn ingest_counted(path: impl Into<String>) -> Self {
        Self::Ingest {
            path: path.into(),
            force: false,
            count_pending: true,
        }
    }

    pub fn rechunk(path: impl Into<String>) -> Self {
        Self::Ingest {
            path: path.into(),
            force: true,
            count_pending: false,
        }
    }

    /// Rechunk job counted by run_wiki_reembed so the pending counter is decremented only for those jobs.
    pub fn rechunk_for_reembed(path: impl Into<String>) -> Self {
        Self::Ingest {
            path: path.into(),
            force: true,
            count_pending: true,
        }
    }
}

pub struct PipelineWorker {
    db_path: PathBuf,
    rx: mpsc::Receiver<PipelineJob>,
    pending: Arc<AtomicUsize>,
    vault_root: Option<PathBuf>,
    status_tx: mpsc::Sender<PipelineStatusEvent>,
}

impl PipelineWorker {
    #[allow(dead_code)]
    pub fn new(db_path: PathBuf, rx: mpsc::Receiver<PipelineJob>, pending: Arc<AtomicUsize>, status_tx: mpsc::Sender<PipelineStatusEvent>) -> Self {
        PipelineWorker { db_path, rx, pending, vault_root: None, status_tx }
    }

    pub fn new_with_vault(
        db_path: PathBuf,
        rx: mpsc::Receiver<PipelineJob>,
        pending: Arc<AtomicUsize>,
        vault_root: Option<PathBuf>,
        status_tx: mpsc::Sender<PipelineStatusEvent>,
    ) -> Self {
        PipelineWorker { db_path, rx, pending, vault_root, status_tx }
    }

    pub fn run(self) {
        let config_path = self
            .db_path
            .parent()
            .map(|p| p.join("config.json"))
            .unwrap_or_else(|| PathBuf::from("config.json"));
        let profile = VaultConfig::new(config_path)
            .get_embed_profile()
            .unwrap_or_else(|err| {
                eprintln!("[pipeline] failed to read embed_profile: {err}, using default");
                crate::embedder::EmbedProfile::default()
            });
        let conn = match Connection::open(&self.db_path) {
            Ok(c) => c,
            Err(err) => {
                eprintln!("[pipeline] db open failed: {err}");
                return;
            }
        };
        if let Err(e) = conn.execute_batch("PRAGMA foreign_keys = ON;") {
            eprintln!("[pipeline] failed to enable FK: {e}");
        }

        let mut pending_linkers = HashSet::new();
        let mut next_job: Option<PipelineJob> = None;

        while let Some(job) = next_job.take().or_else(|| self.rx.recv().ok()) {
            let job_path = match &job {
                PipelineJob::Ingest { path, .. } => path.clone(),
                PipelineJob::Delete(path) => path.clone(),
            };
            let count_pending = matches!(&job, PipelineJob::Ingest { count_pending: true, .. });
            if count_pending {
                let previous = self.pending.fetch_add(1, Ordering::SeqCst);
                let _ = self.status_tx.send(PipelineStatusEvent::PendingCount(previous + 1));
            }
            let worker_vault_root = self.vault_root.clone();
            let mut current_entity: Option<String> = None;

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match job {
                    PipelineJob::Ingest { path, force, .. } => {
                        let vault_root = worker_vault_root.as_deref().or_else(|| {
                            std::path::Path::new(&path)
                                .parent()
                                .and_then(|p| p.parent())
                        }).map(|p| p.to_path_buf());
                        let vault_root_str = worker_vault_root.as_deref()
                            .and_then(|p| p.to_str());
                        match ingest_file(&conn, &profile, &path, force, vault_root_str) {
                            Ok(()) => {
                                let eid = entity_id_for_path(&path, vault_root_str);
                                current_entity = Some(eid.clone());
                                pending_linkers.insert(eid.clone());
                                if let Err(e) = crate::librarian::generate_summary(
                                    &conn,
                                    &path,
                                    crate::setup::recommended_model(),
                                ) {
                                    let msg = format!("librarian error {}: {}", path, e);
                                    eprintln!("[pipeline] {}", msg);
                                    write_error_log(vault_root.as_deref(), &msg);
                                }
                            }
                            Err(e) => {
                                let msg = format!("ingest error {}: {}", path, e);
                                eprintln!("[pipeline] {}", msg);
                                write_error_log(vault_root.as_deref(), &msg);
                            }
                        }
                    }
                    PipelineJob::Delete(path) => {
                        // Remove shadow copy from .brain/converted/ (PDF/DOCX conversion artifact)
                        if let Some(original) = std::path::Path::new(&path).file_stem() {
                            let shadow_root = worker_vault_root.as_deref().or_else(|| {
                                std::path::Path::new(&path)
                                    .parent()
                                    .and_then(|p| p.parent())
                            });
                            if let Some(vault_root) = shadow_root {
                                let shadow = vault_root
                                    .join(".brain")
                                    .join("converted")
                                    .join(format!("{}.md", original.to_string_lossy()));
                                let _ = std::fs::remove_file(&shadow);
                            }
                        }
                        if let Err(e) = delete_document(&conn, &path) {
                            eprintln!("[pipeline] delete error {path}: {e}");
                        }
                        conn.execute(
                            "UPDATE wiki_pages SET status = 'orphaned'
                             WHERE status NOT IN ('rejected', 'orphaned')
                             AND source_doc_ids LIKE ?1",
                            [format!("%{}%", path)],
                        )
                        .ok();
                    }
                }
            }));
            if let Err(e) = result {
                let msg = format!("panic processing {}: {:?}", job_path, e);
                eprintln!("[pipeline] {}", msg);
            }

            if count_pending {
                let updated = self
                    .pending
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                        if current == 0 {
                            Some(0)
                        } else {
                            Some(current - 1)
                        }
                    })
                    .unwrap_or(0);
                let current = updated.saturating_sub(1);
                let _ = self.status_tx.send(PipelineStatusEvent::PendingCount(current));
            }

            let flush_pending_linkers = |conn: &Connection, pending_linkers: &mut HashSet<String>| {
                if pending_linkers.is_empty() {
                    return;
                }
                let since = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0)
                    .saturating_sub(300);
                for eid in pending_linkers.drain() {
                    if let Err(e) = crate::indexer::linker::run_linker(conn, &eid, since) {
                        eprintln!("[linker] run_linker error ({}): {}", eid, e);
                    }
                }
            };

            match self.rx.try_recv() {
                Ok(next) => {
                    let should_flush = match (&current_entity, &next) {
                        (Some(current_eid), PipelineJob::Ingest { path: next_path, .. }) => {
                            let next_vault_root = worker_vault_root.as_deref().and_then(|p| p.to_str());
                            let next_eid = entity_id_for_path(next_path, next_vault_root);
                            &next_eid != current_eid
                        }
                        _ => true,
                    };
                    if should_flush {
                        flush_pending_linkers(&conn, &mut pending_linkers);
                    }
                    next_job = Some(next);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    flush_pending_linkers(&conn, &mut pending_linkers);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    flush_pending_linkers(&conn, &mut pending_linkers);
                    break;
                }
            }
        }

        if !pending_linkers.is_empty() {
            let since = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
                .saturating_sub(300);
            for eid in pending_linkers.drain() {
                if let Err(e) = crate::indexer::linker::run_linker(&conn, &eid, since) {
                    eprintln!("[linker] run_linker error ({}): {}", eid, e);
                }
            }
        }
    }
}

/// Extract text from a binary document format using bundled Rust libraries.
/// Returns `None` for plain-text formats (caller reads bytes directly).
fn extract_text(path: &str) -> Result<Option<String>> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        "pdf" => {
            let text = pdf_extract::extract_text(path)
                .map_err(|e| anyhow::anyhow!("PDF extraction failed: {e}"))?;
            Ok(Some(text))
        }
        "docx" => Ok(Some(extract_docx_text(path)?)),
        _ => Ok(None),
    }
}

fn extract_docx_text(path: &str) -> Result<String> {
    let file = std::fs::File::open(path)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| anyhow::anyhow!("DOCX open failed: {e}"))?;
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(|e| anyhow::anyhow!("word/document.xml not found: {e}"))?
        .read_to_string(&mut xml)?;
    Ok(xml_text_content(&xml))
}

/// Strip XML tags and collapse whitespace, preserving paragraph breaks.
fn xml_text_content(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len() / 4);
    let mut in_tag = false;
    let mut last_was_newline = false;

    for ch in xml.chars() {
        match ch {
            '<' => {
                in_tag = true;
            }
            '>' => {
                in_tag = false;
                if !last_was_newline {
                    out.push(' ');
                    last_was_newline = false;
                }
            }
            _ if !in_tag => {
                out.push(ch);
                last_was_newline = ch == '\n';
            }
            _ => {}
        }
    }

    // Collapse runs of whitespace into single spaces / newlines
    let mut result = String::with_capacity(out.len());
    let mut prev_space = false;
    for ch in out.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                result.push(if ch == '\n' { '\n' } else { ' ' });
            }
            prev_space = true;
        } else {
            result.push(ch);
            prev_space = false;
        }
    }
    result
}

fn write_error_log(vault_path: Option<&std::path::Path>, msg: &str) {
    let Some(vault) = vault_path else {
        return;
    };
    let log_path = vault.join(".brain").join("errors.log");
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("[{}] {}\n", timestamp, msg);
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Runs the normal ingestion path (conversion, chunking, embedding). Same code as the desktop
/// pipeline worker — safe to call inline from tooling (e.g. bulk rechunk).
pub fn ingest_document(
    conn: &Connection,
    profile: &EmbedProfile,
    path: &str,
    force_rechunk: bool,
) -> Result<()> {
    ingest_file(conn, profile, path, force_rechunk, None)
}

pub fn ingest_document_with_vault_root(
    conn: &Connection,
    profile: &EmbedProfile,
    path: &str,
    force_rechunk: bool,
    vault_root: Option<&str>,
) -> Result<()> {
    ingest_file(conn, profile, path, force_rechunk, vault_root)
}

fn normalize_workspace_root(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");
    if normalized != "/" {
        normalized = normalized.trim_end_matches('/').to_string();
        if normalized.ends_with(':') {
            normalized.push('/');
        }
        if normalized.is_empty() {
            normalized = "/".to_string();
        }
    }
    normalized
}

pub fn entity_id_for_path(path: &str, vault_root: Option<&str>) -> String {
    let normalized = std::path::Path::new(path)
        .canonicalize()
        .map(|p| normalize_workspace_root(&p.to_string_lossy()))
        .unwrap_or_else(|_| normalize_workspace_root(path));

    if let Some(root) = vault_root {
        // Strip vault root prefix and inspect only the first vault-relative component so
        // ancestor folders named "documents" or "wiki" (e.g. /Users/me/documents/vault/)
        // or nested sub-folders (e.g. vault/src/wiki/) don't misroute.
        let root_prefix = std::path::Path::new(root)
            .canonicalize()
            .map(|p| normalize_workspace_root(&p.to_string_lossy()))
            .unwrap_or_else(|_| normalize_workspace_root(root));
        let rel = normalized
            .strip_prefix(&format!("{}/", root_prefix))
            .unwrap_or(&normalized);
        let first = rel.split('/').next().unwrap_or("");
        return match first {
            "documents" => "tier_fact".to_string(),
            "wiki" => "tier_wisdom".to_string(),
            _ => {
                let hash = hash_bytes(root_prefix.as_bytes());
                format!("tier_working::{}", &hash[..16])
            }
        };
    }

    // No vault root: fall back to substring heuristics (approximate).
    if normalized.contains("/documents/") {
        "tier_fact".to_string()
    } else if normalized.contains("/wiki/") {
        "tier_wisdom".to_string()
    } else {
        "tier_working".to_string()
    }
}

fn ingest_file(
    conn: &Connection,
    profile: &EmbedProfile,
    path: &str,
    force_rechunk: bool,
    vault_root: Option<&str>,
) -> Result<()> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if !should_ingest_extension(ext) {
        return Ok(());
    }

    let raw_bytes = std::fs::read(path)?;
    let hash = hash_bytes(&raw_bytes);

    if let Some(doc) = get_document_by_path(conn, path)? {
        if !force_rechunk && doc.hash == hash && doc.status == "indexed" {
            return Ok(());
        }
        delete_document_chunks(conn, doc.id)?;
    }

    let text = match extract_text(path)? {
        Some(t) => t,
        None => String::from_utf8_lossy(&raw_bytes).into_owned(),
    };

    let doc_id = upsert_document(conn, path, &hash)?;
    let eid = entity_id_for_path(path, vault_root);

    let mut chunks = chunk_autodetect(Path::new(path), &text);

    // Pass 2: extract reference/call-site chunks for supported code files
    let strategy = crate::chunker::classify(Path::new(path));
    if let ChunkStrategy::AstSymbol(ast_lang) = strategy {
        let ref_lang = match ast_lang {
            AstLang::Rust => RefLang::Rust,
            AstLang::TypeScript => RefLang::TypeScript,
            AstLang::JavaScript => RefLang::JavaScript,
            AstLang::Python => RefLang::Python,
            AstLang::Go => RefLang::Go,
        };
        let refs = extract_references(ref_lang, &text, 0);
        chunks.extend(refs);
    }

    if chunks.is_empty() {
        mark_document_indexed(conn, doc_id)?;
        return Ok(());
    }

    let texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();

    let embeddings = embed_batch(profile, texts).inspect_err(|_| {
        let _ = mark_document_error(conn, doc_id);
    })?;

    for (i, (chunk, vector)) in chunks.iter().zip(embeddings.iter()).enumerate() {
        let chunk_id = insert_chunk(conn, doc_id, chunk, i, &eid)?;
        insert_embedding(conn, chunk_id, vector)?;
    }

    mark_document_indexed(conn, doc_id)?;
    Ok(())
}

pub fn start_pipeline(
    db_path: PathBuf,
    vault_root: Option<PathBuf>,
) -> (
    mpsc::SyncSender<PipelineJob>,
    std::thread::JoinHandle<()>,
    Arc<AtomicUsize>,
    Option<mpsc::Receiver<PipelineStatusEvent>>,
) {
    let (tx, rx) = mpsc::sync_channel::<PipelineJob>(256);
    let (status_tx, status_rx) = mpsc::channel();
    let pending = Arc::new(AtomicUsize::new(0));
    let worker = PipelineWorker::new_with_vault(db_path, rx, pending.clone(), vault_root, status_tx);
    let join = std::thread::Builder::new()
        .name("pipeline-worker".to_string())
        .spawn(move || worker.run())
        .expect("spawn pipeline worker");
    (tx, join, pending, Some(status_rx))
}

#[cfg(test)]
mod pass_integration_tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use crate::db::queries::{insert_chunk, upsert_document};

    #[test]
    fn ingest_rust_file_produces_def_and_ref_chunks() {
        let tmp = tempfile::TempDir::new().unwrap();
        let rust_file = tmp.path().join("documents").join("init.rs");
        std::fs::create_dir_all(rust_file.parent().unwrap()).unwrap();
        std::fs::write(
            &rust_file,
            r#"
fn init_db() {
    connect();
}
fn connect() {}
"#,
        )
        .unwrap();

        let conn = open_in_memory().unwrap();
        let path_str = rust_file.to_string_lossy().to_string();

        let doc_id = upsert_document(&conn, &path_str, "testhash").unwrap();

        let text = std::fs::read_to_string(&rust_file).unwrap();
        let eid = entity_id_for_path(&path_str, None);

        let mut chunks = crate::chunker::chunk_autodetect(&rust_file, &text);

        let strategy = crate::chunker::classify(&rust_file);
        if let crate::chunker::ChunkStrategy::AstSymbol(ast_lang) = strategy {
            let ref_lang = match ast_lang {
                crate::chunker::AstLang::Rust => crate::indexer::RefLang::Rust,
                _ => return,
            };
            chunks.extend(crate::indexer::extract_references(ref_lang, &text, 0));
        }

        for (i, chunk) in chunks.iter().enumerate() {
            insert_chunk(&conn, doc_id, chunk, i, &eid).unwrap();
        }

        let def_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE defined_symbol IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(def_count > 0, "expected at least one definition chunk");

        let ref_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE strategy = 'ast_ref' AND symbol_name IS NOT NULL AND defined_symbol IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            ref_count > 0,
            "expected at least one reference chunk from Pass 2"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_text_skips_markdown() {
        // plain-text formats return None so caller handles them
        assert!(extract_text("/vault/documents/note.md").unwrap().is_none());
        assert!(extract_text("/vault/documents/note.txt").unwrap().is_none());
    }

    #[test]
    fn test_extract_text_skips_unknown() {
        assert!(extract_text("/vault/documents/image.png")
            .unwrap()
            .is_none());
        assert!(extract_text("/vault/documents/data.csv").unwrap().is_none());
    }

    #[test]
    fn rechunk_does_not_count_pending_by_default() {
        match PipelineJob::rechunk("/vault/documents/note.md") {
            PipelineJob::Ingest { count_pending, .. } => assert!(!count_pending),
            _ => panic!("expected PipelineJob::Ingest variant"),
        }
    }

    #[test]
    fn rechunk_for_reembed_counts_pending() {
        match PipelineJob::rechunk_for_reembed("/vault/documents/note.md") {
            PipelineJob::Ingest { count_pending, .. } => assert!(count_pending),
            _ => panic!("expected PipelineJob::Ingest variant"),
        }
    }

    #[test]
    fn test_xml_text_content_strips_tags() {
        let xml = "<w:p><w:r><w:t>Hello</w:t></w:r><w:r><w:t> world</w:t></w:r></w:p>";
        let text = xml_text_content(xml);
        assert!(text.contains("Hello"));
        assert!(text.contains("world"));
        assert!(!text.contains('<'));
    }

    #[test]
    fn entity_id_for_path_normalizes_vault_root_like_workspace_id() {
        let id_a = entity_id_for_path(
            "/Users/foo/Vault/src/db.rs",
            Some("/Users/foo/Vault"),
        );
        let id_b = entity_id_for_path(
            "/Users/foo/Vault/src/db.rs",
            Some("/Users/foo/Vault/"),
        );
        assert_eq!(id_a, id_b);
    }
}
