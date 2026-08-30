//! Tests for pipeline worker config-loading behavior.
//!
//! The pipeline worker must use the unified `BrainConfig::load_lenient()`
//! loader and hard-fail (not silently default to an unconfigured LLM) when
//! `config.json` is malformed.  Silently defaulting is Problem class 2: it
//! routes every embedding through an unconfigured provider and looks like an
//! onboarding reset to the user.

use std::sync::{atomic::AtomicUsize, mpsc, Arc};

use tauri_app_lib::config::BrainConfig;
use tauri_app_lib::retrieval::BrainPaths;
use tauri_app_lib::PipelineJob;
use tempfile::TempDir;

static PIPELINE_STUB_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Lenient load returns a typed ConfigError on parse failure.
///
/// load_lenient's contract changed (PR #120 follow-up): malformed JSON is
/// propagated as `Err`, not as a diagnostic string inside `LoadReport`. The
/// pipeline worker propagates that error and exits at startup.
#[test]
fn load_lenient_reports_malformed_on_broken_json() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");
    std::fs::write(&config_path, "{ truncated }").unwrap();

    let paths = BrainPaths {
        brain_dir: temp.path().to_path_buf(),
        config_path,
        db_path: temp.path().join("brain.db"),
    };

    let result = BrainConfig::load_lenient(&paths);
    assert!(
        result.is_err(),
        "expected Err for broken JSON, got {:?}",
        result
    );
}

/// Pipeline worker must refuse to run on malformed JSON config.
///
/// Without this guard the worker would silently fall back to
/// `EmbedProfile::default()`, route embeddings through an unconfigured LLM,
/// and present to the user as if their config had been wiped.
#[test]
fn pipeline_worker_exits_early_on_malformed_config() {
    let _stub_lock = PIPELINE_STUB_GUARD.lock().unwrap();
    std::env::set_var("CURATED_EMBED_STUB", "constant8");
    struct StubUnset;
    impl Drop for StubUnset {
        fn drop(&mut self) {
            std::env::remove_var("CURATED_EMBED_STUB");
        }
    }
    let _stub_cleanup = StubUnset;

    let tmp = TempDir::new().unwrap();
    // Initialise the DB so the worker has something to open.
    drop(tauri_app_lib::make_test_app(tmp.path()));
    let db_path = tmp.path().join("brain.db");

    // Write a deliberately malformed config.json next to the db_path.
    let config_path = db_path.parent().unwrap().join("config.json");
    std::fs::write(&config_path, "{ truncated }").unwrap();

    // Spawn the pipeline worker. It must die on startup because the config
    // cannot be parsed.
    let (tx, rx) = mpsc::sync_channel::<PipelineJob>(4);
    let (status_tx, _status_rx) = mpsc::channel();
    let worker = tauri_app_lib::PipelineWorker::new(
        db_path.clone(),
        rx,
        Arc::new(AtomicUsize::new(0)),
        status_tx,
    );
    let handle = std::thread::Builder::new()
        .name("pipeline-worker-malformed".to_string())
        .spawn(move || worker.run())
        .expect("spawn pipeline worker");

    // The worker should exit promptly — no jobs were sent and it bailed
    // before entering the job loop. Use a short timeout so a regression that
    // made the worker hang would fail loudly.
    let joined = wait_with_timeout(handle, std::time::Duration::from_secs(5));
    assert!(
        joined.is_ok(),
        "pipeline worker did not exit within 5s after malformed config"
    );

    // No documents should have been indexed because the worker died at
    // startup before processing any jobs.
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let doc_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        doc_count, 0,
        "pipeline worker indexed documents despite malformed config"
    );

    // Sanity check: drop the (already-closed) sender so the test does not
    // leak resources.
    drop(tx);
}

/// Wait for a thread handle to finish, returning an error if it takes longer
/// than `timeout`.  Used to catch regressions where the pipeline worker
/// would block forever instead of exiting on malformed config.
fn wait_with_timeout(
    handle: std::thread::JoinHandle<()>,
    timeout: std::time::Duration,
) -> Result<(), &'static str> {
    let (done_tx, done_rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let _ = handle.join();
        let _ = done_tx.send(());
    });
    done_rx
        .recv_timeout(timeout)
        .map(|_| ())
        .map_err(|_| "worker thread did not finish before timeout")
}
