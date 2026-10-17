mod helpers;

use std::sync::{atomic::AtomicUsize, mpsc, Arc, Mutex};

use tauri_app_lib::{Heartbeat, PipelineJob, PipelineWorker};
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
    let (tx, rx) = mpsc::sync_channel::<PipelineJob>(64);
    let (status_tx, _status_rx) = mpsc::channel();
    let worker = PipelineWorker::new(
        db_path,
        rx,
        Arc::new(AtomicUsize::new(0)),
        status_tx,
        Arc::new(Heartbeat::new()),
    );
    let handle = std::thread::spawn(move || worker.run());
    for job in jobs {
        tx.send(job).unwrap();
    }
    drop(tx); // closes channel; worker exits after draining
    handle.join().expect("pipeline worker panicked");
}

#[test]
fn ingest_skips_vault_wiki_markdown_under_vault_root() {
    let tmp = TempDir::new().unwrap();
    let vault = tmp.path().join("vault");
    let wiki_file = vault.join("wiki").join("legacy.md");
    std::fs::create_dir_all(wiki_file.parent().unwrap()).unwrap();
    std::fs::write(&wiki_file, "# Legacy\n\n".to_owned() + &"word ".repeat(20)).unwrap();

    let _stub_lock = PIPELINE_STUB_GUARD.lock().unwrap();
    std::env::set_var("CURATED_EMBED_STUB", "constant8");
    let _stub_cleanup = StubUnset;

    drop(tauri_app_lib::make_test_app(tmp.path()));

    let db_path = tmp.path().join("brain.db");
    let (tx, rx) = mpsc::sync_channel::<PipelineJob>(64);
    let (status_tx, _status_rx) = mpsc::channel();
    let worker = PipelineWorker::new_with_vault(
        db_path.clone(),
        rx,
        Arc::new(AtomicUsize::new(0)),
        Some(vault.clone()),
        status_tx,
        Arc::new(Heartbeat::new()),
    );
    let handle = std::thread::spawn(move || worker.run());
    tx.send(PipelineJob::ingest(wiki_file.to_string_lossy().to_string()))
        .unwrap();
    drop(tx);
    handle.join().expect("pipeline worker panicked");

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0, "vault/wiki/*.md must not be indexed post-V7");
}

#[test]
fn ingest_markdown_indexes_document_with_chunks_and_embeddings() {
    let tmp = TempDir::new().unwrap();
    let docs_dir = tmp.path().join("documents");
    std::fs::create_dir_all(&docs_dir).unwrap();

    // Write a document with enough words for at least one chunk
    let doc_path = docs_dir.join("note.md");
    std::fs::write(
        &doc_path,
        "# Test Note\n\n".to_owned() + &"word ".repeat(20),
    )
    .unwrap();

    run_pipeline_job(
        &tmp,
        vec![PipelineJob::ingest(doc_path.to_string_lossy().to_string())],
    );

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
    assert_eq!(
        embedding_count, chunk_count,
        "embedding count should match chunk count"
    );
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
    assert_eq!(
        count_before, count_after,
        "chunks changed despite file being unchanged"
    );

    run_pipeline_job(&tmp, vec![PipelineJob::rechunk(path_str)]);

    let count_force: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        count_force, count_after,
        "strategy-stable rechunk preserves chunk count"
    );
    // Issue #271 §8(a) diff-swap: a LOSSLESS forced rechunk (same chunker,
    // unchanged bytes) keeps every chunk_id — the swap UPDATEs the embeddings
    // rows in place instead of delete-then-reinsert, so chunk identity (and
    // every evidence hash anchored to it) survives the pass. The old
    // "force rechunk should replace chunk rows" expectation pinned the
    // destructive pre-swap behavior.
    let max_id_after: i64 = conn
        .query_row("SELECT COALESCE(MAX(id), 0) FROM chunks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        max_id_after, max_id_before,
        "lossless force rechunk must keep chunk identity (diff-swap §8(a))"
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
    std::fs::write(
        &doc_path,
        "completely different new content for re-indexing",
    )
    .unwrap();
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
    assert_eq!(
        hash, expected_hash,
        "document hash not updated after re-ingest"
    );
}

#[test]
fn unsupported_extension_not_indexed() {
    let tmp = TempDir::new().unwrap();
    let docs_dir = tmp.path().join("documents");
    std::fs::create_dir_all(&docs_dir).unwrap();

    let img_path = docs_dir.join("photo.png");
    std::fs::write(&img_path, b"\x89PNG fake image data").unwrap();

    run_pipeline_job(
        &tmp,
        vec![PipelineJob::ingest(img_path.to_string_lossy().to_string())],
    );

    let conn = rusqlite::Connection::open(tmp.path().join("brain.db")).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0, "unsupported extension should not be indexed");
}

// ── pr283-review fix round (findings 9/10/11) ───────────────────────────────

fn chunk_count(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
        .unwrap()
}

/// pr283-review finding 9: the zero-chunk branch only wrote
/// `last_indexed_hash`, leaving `documents.hash` stale — content that later
/// came back byte-identical hit the `doc.hash == hash` short-circuit and the
/// doc stayed chunkless. After the fix the branch writes the new hash, so
/// the restored content re-ingests through the diff-swap and the chunks come
/// back.
#[test]
fn zero_chunk_reingest_updates_documents_hash_for_short_circuit() {
    let tmp = TempDir::new().unwrap();
    let docs_dir = tmp.path().join("documents");
    std::fs::create_dir_all(&docs_dir).unwrap();
    let doc = docs_dir.join("zero.rs");
    let with_chunks = "fn alpha() { let a = 1; }\n";
    std::fs::write(&doc, with_chunks).unwrap();
    let path = doc.to_string_lossy().to_string();

    run_pipeline_job(&tmp, vec![PipelineJob::ingest(path.clone())]);
    let db_path = tmp.path().join("brain.db");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        assert!(
            chunk_count(&conn) > 0,
            "fixture: the first ingest must produce chunks"
        );
    }

    // Content → zero chunks: the doc re-ingests through the zero-chunk
    // branch (chunks dropped, marked indexed).
    std::fs::write(&doc, "").unwrap();
    run_pipeline_job(&tmp, vec![PipelineJob::ingest(path.clone())]);
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        assert_eq!(chunk_count(&conn), 0, "zero-chunk edit drops the chunks");
    }

    // The original content comes back: the short-circuit must NOT fire (the
    // stored hash now describes the empty file, not this content).
    std::fs::write(&doc, with_chunks).unwrap();
    run_pipeline_job(&tmp, vec![PipelineJob::ingest(path.clone())]);
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    assert!(
        chunk_count(&conn) > 0,
        "restored content must re-ingest (documents.hash was updated by the zero-chunk branch)"
    );
}

/// pr283-review finding 10: an unforced edit that ONLY removes chunks leaves
/// `need_embed` empty — no embed round-trip may happen (an external profile
/// can error on the empty batch's key resolution), while the swap still
/// removes the dead chunks.
#[test]
fn unforced_edit_that_only_removes_chunks_skips_the_embed_round_trip() {
    let tmp = TempDir::new().unwrap();
    // pr283-review round 2: the count file lives in THIS test's unique
    // TempDir — a fixed shared temp path collides with parallel tests that
    // also embed between the n_seed and n_after reads.
    let counts = tmp.path().join("embed-counts.log");
    std::fs::remove_file(&counts).ok();
    let docs_dir = tmp.path().join("documents");
    std::fs::create_dir_all(&docs_dir).unwrap();
    let doc = docs_dir.join("shrink.rs");
    let two_fns = "fn alpha() { let a = 1; }\n\nfn beta() -> u32 { alpha() }\n";
    let one_fn = "fn alpha() { let a = 1; }\n";
    std::fs::write(&doc, two_fns).unwrap();
    let path = doc.to_string_lossy().to_string();

    // SAFETY: single-threaded stub section (PIPELINE_STUB_GUARD held inside
    // run_pipeline_job per call); the count file is unique to this test.
    std::env::set_var("CURATED_EMBED_COUNT_FILE", counts.to_str().unwrap());
    struct CountUnset;
    impl Drop for CountUnset {
        fn drop(&mut self) {
            std::env::remove_var("CURATED_EMBED_COUNT_FILE");
        }
    }
    let _count_cleanup = CountUnset;

    run_pipeline_job(&tmp, vec![PipelineJob::ingest(path.clone())]);
    let n_seed = std::fs::read_to_string(&counts)
        .unwrap_or_default()
        .lines()
        .count();
    let db_path = tmp.path().join("brain.db");
    let seed_chunks = {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        chunk_count(&conn)
    };
    assert!(seed_chunks >= 2, "fixture: two fns + a ref chunk expected");

    // Remove beta (and its call-site ref chunk); alpha is byte-identical ⇒
    // every surviving chunk is KEPT ⇒ need_embed is empty.
    std::fs::write(&doc, one_fn).unwrap();
    run_pipeline_job(&tmp, vec![PipelineJob::ingest(path.clone())]);

    let n_after = std::fs::read_to_string(&counts)
        .unwrap_or_default()
        .lines()
        .count();
    assert_eq!(
        n_after, n_seed,
        "an edit that only removes chunks must not call the embedder at all"
    );
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    assert!(
        chunk_count(&conn) < seed_chunks,
        "the swap must still remove the dead chunks"
    );
    std::fs::remove_file(&counts).ok();
}

/// pr283-review finding 11: compute_chunk_hash covers text/path/position
/// only — an unforced edit that adds lines shifts later chunks' line spans
/// while their hashes still match. KEPT chunks must get their metadata
/// (start_line/end_line, …) refreshed from the new chunk list.
#[test]
fn kept_chunks_get_fresh_line_spans_after_unforced_edit() {
    let tmp = TempDir::new().unwrap();
    let docs_dir = tmp.path().join("documents");
    std::fs::create_dir_all(&docs_dir).unwrap();
    let doc = docs_dir.join("spans.rs");
    let v1 = "fn alpha() { let a = 1; }\nfn beta() -> u32 { alpha() }\n";
    // v2 adds a LINE inside alpha (its text changes → re-embed) so beta's
    // span shifts by +1 while beta's text stays byte-identical (KEPT).
    let v2 = "fn alpha() { let a = 1;\n    let b = 2; }\nfn beta() -> u32 { alpha() }\n";
    std::fs::write(&doc, v1).unwrap();
    let path = doc.to_string_lossy().to_string();

    run_pipeline_job(&tmp, vec![PipelineJob::ingest(path.clone())]);
    let db_path = tmp.path().join("brain.db");
    let (chunk_id, old_start, old_end, old_text): (i64, i64, i64, String) = {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.query_row(
            "SELECT id, start_line, end_line, chunk_text FROM chunks \
             WHERE symbol_name = 'beta' ORDER BY id LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
    };
    assert!(
        old_start >= 2,
        "fixture: beta must start past line 1, got {old_start}"
    );

    // Unforced edit: alpha's text changes (re-embed), beta's text is
    // byte-identical (KEPT) — but its line span shifted by +1.
    std::fs::write(&doc, v2).unwrap();
    run_pipeline_job(&tmp, vec![PipelineJob::ingest(path.clone())]);

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let (new_start, new_end, new_text): (i64, i64, String) = conn
        .query_row(
            "SELECT start_line, end_line, chunk_text FROM chunks WHERE id = ?1",
            [chunk_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(new_text, old_text, "the chunk is KEPT (same hash)");
    assert_eq!(
        new_start,
        old_start + 1,
        "kept chunk's start_line must follow the edit (+1 line added above it)"
    );
    assert!(
        new_end >= new_start,
        "kept chunk's end_line must be a fresh, sane span"
    );
    assert_eq!(
        new_end,
        old_end + 1,
        "kept chunk's end_line must follow the edit too"
    );
}
