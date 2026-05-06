use anyhow::Result;
use rusqlite::Connection;
use std::{path::{Path, PathBuf}, sync::mpsc};

use crate::chunker::chunk_text;
use crate::db::queries::{
    delete_document, delete_document_chunks, get_document_by_path, insert_chunk,
    insert_embedding, mark_document_error, mark_document_indexed, upsert_document,
};
use crate::embedder::Embedder;
use crate::hasher::hash_bytes;

#[derive(Debug, Clone)]
pub enum PipelineJob {
    Ingest(String),
    Delete(String),
}

pub struct PipelineWorker {
    db_path: PathBuf,
    rx: mpsc::Receiver<PipelineJob>,
}

impl PipelineWorker {
    pub fn new(db_path: PathBuf, rx: mpsc::Receiver<PipelineJob>) -> Self {
        PipelineWorker { db_path, rx }
    }

    pub fn run(self) {
        let embedder = match Embedder::new() {
            Ok(e) => e,
            Err(err) => {
                eprintln!("[pipeline] embedder init failed: {err}");
                return;
            }
        };
        let conn = match Connection::open(&self.db_path) {
            Ok(c) => c,
            Err(err) => {
                eprintln!("[pipeline] db open failed: {err}");
                return;
            }
        };

        for job in self.rx {
            match job {
                PipelineJob::Ingest(path) => {
                    if let Err(e) = ingest_file(&conn, &embedder, &path) {
                        eprintln!("[pipeline] ingest error {path}: {e}");
                    }
                }
                PipelineJob::Delete(path) => {
                    if let Err(e) = delete_document(&conn, &path) {
                        eprintln!("[pipeline] delete error {path}: {e}");
                    }
                }
            }
        }
    }
}

fn ingest_file(conn: &Connection, embedder: &Embedder, path: &str) -> Result<()> {
    let p = Path::new(path);
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !matches!(ext, "md" | "txt" | "markdown") {
        return Ok(());
    }

    let bytes = std::fs::read(path)?;
    let hash = hash_bytes(&bytes);

    if let Some(doc) = get_document_by_path(conn, path)? {
        if doc.hash == hash && doc.status == "indexed" {
            return Ok(());
        }
        delete_document_chunks(conn, doc.id)?;
    }

    let text = String::from_utf8_lossy(&bytes).to_string();
    let doc_id = upsert_document(conn, path, &hash)?;

    let chunks = chunk_text(&text);
    if chunks.is_empty() {
        mark_document_indexed(conn, doc_id)?;
        return Ok(());
    }

    let embeddings = embedder.embed(chunks.clone()).map_err(|e| {
        let _ = mark_document_error(conn, doc_id);
        e
    })?;

    for (i, (chunk, vector)) in chunks.iter().zip(embeddings.iter()).enumerate() {
        let chunk_id = insert_chunk(conn, doc_id, chunk, i)?;
        insert_embedding(conn, chunk_id, vector)?;
    }

    mark_document_indexed(conn, doc_id)?;
    Ok(())
}

pub fn start_pipeline(db_path: PathBuf) -> mpsc::SyncSender<PipelineJob> {
    let (tx, rx) = mpsc::sync_channel::<PipelineJob>(256);
    let worker = PipelineWorker::new(db_path, rx);
    std::thread::Builder::new()
        .name("pipeline-worker".to_string())
        .spawn(move || worker.run())
        .expect("spawn pipeline worker");
    tx
}
