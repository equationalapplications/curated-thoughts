use anyhow::Result;
use rusqlite::Connection;
use std::{
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
    Ingest {
        path: String,
        force: bool,
    },
    Delete(String),
}

impl PipelineJob {
    pub fn ingest(path: impl Into<String>) -> Self {
        Self::Ingest {
            path: path.into(),
            force: false,
        }
    }

    pub fn rechunk(path: impl Into<String>) -> Self {
        Self::Ingest {
            path: path.into(),
            force: true,
        }
    }
}

pub struct PipelineWorker {
    db_path: PathBuf,
    rx: mpsc::Receiver<PipelineJob>,
    pending: Arc<AtomicUsize>,
}

impl PipelineWorker {
    pub fn new(db_path: PathBuf, rx: mpsc::Receiver<PipelineJob>, pending: Arc<AtomicUsize>) -> Self {
        PipelineWorker { db_path, rx, pending }
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

        for job in self.rx {
            let job_path = match &job {
                PipelineJob::Ingest { path, .. } => path.clone(),
                PipelineJob::Delete(path) => path.clone(),
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match job {
                    PipelineJob::Ingest { path, force } => {
                        let vault_root = std::path::Path::new(&path)
                            .parent()
                            .and_then(|p| p.parent())
                            .map(|p| p.to_path_buf());
                        match ingest_document(&conn, &profile, &path, force) {
                            Ok(()) => {
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
                            if let Some(vault_root) = std::path::Path::new(&path)
                                .parent()
                                .and_then(|p| p.parent())
                            {
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

            let _ = self.pending.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                if current == 0 {
                    Some(0)
                } else {
                    Some(current - 1)
                }
            });
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
    ingest_file(conn, profile, path, force_rechunk)
}

fn entity_id_for_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
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
    let eid = entity_id_for_path(path);

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

pub fn start_pipeline(db_path: PathBuf) -> (mpsc::SyncSender<PipelineJob>, std::thread::JoinHandle<()>, Arc<AtomicUsize>) {
    let (tx, rx) = mpsc::sync_channel::<PipelineJob>(256);
    let pending = Arc::new(AtomicUsize::new(0));
    let worker = PipelineWorker::new(db_path, rx, pending.clone());
    let join = std::thread::Builder::new()
        .name("pipeline-worker".to_string())
        .spawn(move || worker.run())
        .expect("spawn pipeline worker");
    (tx, join, pending)
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
        let eid = entity_id_for_path(&path_str);

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
    fn test_xml_text_content_strips_tags() {
        let xml = "<w:p><w:r><w:t>Hello</w:t></w:r><w:r><w:t> world</w:t></w:r></w:p>";
        let text = xml_text_content(xml);
        assert!(text.contains("Hello"));
        assert!(text.contains("world"));
        assert!(!text.contains('<'));
    }
}
