mod helpers;
use std::sync::{atomic::AtomicUsize, mpsc, Arc};
use tauri_app_lib::{PipelineJob, PipelineWorker};
use tempfile::TempDir;

fn migrate_db(tmp: &TempDir) {
    // make_test_app runs migrations then drops — leaves migrated DB on disk
    drop(tauri_app_lib::make_test_app(tmp.path()));
}

fn seed_document_with_embedding(conn: &rusqlite::Connection, path: &str) -> (i64, i64) {
    conn.execute(
        "INSERT INTO documents (path, hash, tier, status) VALUES (?1, 'testhash', 'user_doc', 'indexed')",
        [path],
    ).unwrap();
    let doc_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO chunks (doc_id, chunk_text, position) VALUES (?1, 'test chunk', 0)",
        [doc_id],
    )
    .unwrap();
    let chunk_id = conn.last_insert_rowid();
    let vector: Vec<u8> = vec![0u8; 384 * 4]; // 384 zero f32s
    conn.execute(
        "INSERT INTO embeddings (chunk_id, vector) VALUES (?1, ?2)",
        rusqlite::params![chunk_id, vector],
    )
    .unwrap();
    (doc_id, chunk_id)
}

#[test]
fn delete_cascades_chunks_embeddings_shadow_copy_and_orphans_wiki() {
    let tmp = TempDir::new().unwrap();
    migrate_db(&tmp);

    let docs_dir = tmp.path().join("documents");
    let converted_dir = tmp.path().join(".brain").join("converted");
    std::fs::create_dir_all(&docs_dir).unwrap();
    std::fs::create_dir_all(&converted_dir).unwrap();

    let doc_path = docs_dir.join("report.pdf");
    std::fs::write(&doc_path, b"fake pdf").unwrap();
    let doc_path_str = doc_path.to_string_lossy().to_string();

    // Shadow copy in .brain/converted/
    let shadow = converted_dir.join("report.md");
    std::fs::write(&shadow, "# Converted").unwrap();

    let conn = rusqlite::Connection::open(tmp.path().join("brain.db")).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    let (doc_id, _) = seed_document_with_embedding(&conn, &doc_path_str);

    // Seed a wiki page sourced from this document
    conn.execute(
        "INSERT INTO wiki_pages (path, source_doc_ids, generated_by, status) VALUES ('report.md', ?1, 'test', 'approved')",
        [serde_json::json!([doc_path_str]).to_string()],
    ).unwrap();

    drop(conn); // release before pipeline opens its own connection

    let db_path = tmp.path().join("brain.db");
    let (tx, rx) = mpsc::sync_channel::<PipelineJob>(4);
    let (status_tx, _status_rx) = mpsc::channel();
    let worker = PipelineWorker::new(
        db_path.clone(),
        rx,
        Arc::new(AtomicUsize::new(0)),
        status_tx,
    );
    let handle = std::thread::spawn(move || worker.run());
    tx.send(PipelineJob::Delete(doc_path_str.clone())).unwrap();
    drop(tx);
    handle.join().unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

    // Document row gone
    let doc_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM documents WHERE id = ?1",
            [doc_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(doc_count, 0, "document row not deleted");

    // Chunks cascade-deleted
    let chunk_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(chunk_count, 0, "chunks not cascade-deleted");

    // Embeddings cascade-deleted
    let emb_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(emb_count, 0, "embeddings not cascade-deleted");

    // Shadow copy removed
    assert!(
        !shadow.exists(),
        "shadow copy not removed from .brain/converted/"
    );

    // Wiki page orphaned
    let wiki_status: String = conn
        .query_row(
            "SELECT status FROM wiki_pages WHERE path = 'report.md'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(wiki_status, "orphaned", "wiki page not marked orphaned");
}
