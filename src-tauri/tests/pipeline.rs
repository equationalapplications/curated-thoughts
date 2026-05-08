mod helpers;

use std::sync::Mutex;

use tauri_app_lib::{PipelineJob, PipelineWorker};
use tempfile::TempDir;

static PIPELINE_STUB_GUARD: Mutex<()> = Mutex::new(());

struct StubUnset;
impl Drop for StubUnset {
    fn drop(&mut self) {
        std::env::remove_var("CURATED_EMBED_STUB");
    }
}

fn run_pipeline_job(tmp: &TempDir, jobs: Vec<PipelineJob>) {
    let _stub_lock = PIPELINE_STUB_GUARD.lock().unwrap();
    std::env::set_var("CURATED_EMBED_STUB", "constant8");
    let _stub_cleanup = StubUnset;

    // make_test_app opens the DB (running migrations), then drops it immediately.
    drop(tauri_app_lib::make_test_app(tmp.path()));

    let db_path = tmp.path().join("brain.db");
    let (tx, rx) = std::sync::mpsc::sync_channel::<PipelineJob>(64);
    let worker = PipelineWorker::new(db_path, rx);
    let handle = std::thread::spawn(move || worker.run());
    for job in jobs {
        tx.send(job).unwrap();
    }
    drop(tx); // closes channel; worker exits after draining
    handle.join().expect("pipeline worker panicked");
}

#[test]
fn ingest_markdown_indexes_document_with_chunks_and_embeddings() {
    let tmp = TempDir::new().unwrap();
    let docs_dir = tmp.path().join("documents");
    std::fs::create_dir_all(&docs_dir).unwrap();

    // Write a document with enough words for at least one chunk
    let doc_path = docs_dir.join("note.md");
    std::fs::write(&doc_path, "# Test Note\n\n".to_owned() + &"word ".repeat(20)).unwrap();

    run_pipeline_job(&tmp, vec![PipelineJob::ingest(doc_path.to_string_lossy().to_string())]);

    let conn = rusqlite::Connection::open(tmp.path().join("brain.db")).unwrap();

    let status: String = conn
        .query_row(
            "SELECT status FROM documents WHERE path = ?1",
            [doc_path.to_string_lossy().to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "indexed");

    let chunk_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
        .unwrap();
    assert!(chunk_count > 0, "no chunks created");

    let embedding_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(embedding_count, chunk_count, "embedding count should match chunk count");
}

#[test]
fn ingest_same_file_twice_unchanged_does_not_reindex() {
    let tmp = TempDir::new().unwrap();
    let docs_dir = tmp.path().join("documents");
    std::fs::create_dir_all(&docs_dir).unwrap();

    let doc_path = docs_dir.join("stable.md");
    std::fs::write(&doc_path, "stable content").unwrap();
    let path_str = doc_path.to_string_lossy().to_string();

    run_pipeline_job(&tmp, vec![PipelineJob::ingest(path_str.clone())]);

    let conn = rusqlite::Connection::open(tmp.path().join("brain.db")).unwrap();
    let count_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
        .unwrap();
    let max_id_before: i64 = conn
        .query_row("SELECT COALESCE(MAX(id), 0) FROM chunks", [], |r| r.get(0))
        .unwrap();

    // Ingest again without changing the file
    run_pipeline_job(&tmp, vec![PipelineJob::ingest(path_str.clone())]);

    let count_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count_before, count_after, "chunks changed despite file being unchanged");

    run_pipeline_job(&tmp, vec![PipelineJob::rechunk(path_str)]);

    let count_force: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        count_force, count_after,
        "strategy-stable rechunk preserves chunk count"
    );
    let max_id_after: i64 = conn
        .query_row("SELECT COALESCE(MAX(id), 0) FROM chunks", [], |r| r.get(0))
        .unwrap();
    assert!(
        max_id_after > max_id_before,
        "force rechunk should replace chunk rows"
    );
}

#[test]
fn ingest_changed_file_replaces_old_chunks() {
    let tmp = TempDir::new().unwrap();
    let docs_dir = tmp.path().join("documents");
    std::fs::create_dir_all(&docs_dir).unwrap();

    let doc_path = docs_dir.join("changing.md");
    std::fs::write(&doc_path, "original content").unwrap();
    let path_str = doc_path.to_string_lossy().to_string();

    run_pipeline_job(&tmp, vec![PipelineJob::ingest(path_str.clone())]);

    // Change file content
    std::fs::write(&doc_path, "completely different new content for re-indexing").unwrap();
    run_pipeline_job(&tmp, vec![PipelineJob::ingest(path_str.clone())]);

    let conn = rusqlite::Connection::open(tmp.path().join("brain.db")).unwrap();
    let hash: String = conn
        .query_row(
            "SELECT hash FROM documents WHERE path = ?1",
            [&path_str],
            |r| r.get(0),
        )
        .unwrap();

    // Hash should be the new file's hash, not the original
    use sha2::{Digest, Sha256};
    let new_content = std::fs::read(&doc_path).unwrap();
    let expected_hash = hex::encode(Sha256::digest(&new_content));
    assert_eq!(hash, expected_hash, "document hash not updated after re-ingest");
}

#[test]
fn unsupported_extension_not_indexed() {
    let tmp = TempDir::new().unwrap();
    let docs_dir = tmp.path().join("documents");
    std::fs::create_dir_all(&docs_dir).unwrap();

    let img_path = docs_dir.join("photo.png");
    std::fs::write(&img_path, b"\x89PNG fake image data").unwrap();

    run_pipeline_job(&tmp, vec![PipelineJob::ingest(img_path.to_string_lossy().to_string())]);

    let conn = rusqlite::Connection::open(tmp.path().join("brain.db")).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0, "unsupported extension should not be indexed");
}
