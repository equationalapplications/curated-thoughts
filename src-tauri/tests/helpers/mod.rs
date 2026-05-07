#[cfg(feature = "slow-tests")]
pub mod recall_bench;

use serde::de::DeserializeOwned;
use serde_json::Value;
use tauri::{test::MockRuntime, WebviewUrl, WebviewWindowBuilder};
use tauri_app_lib::make_test_app;
use tempfile::TempDir;

pub struct TestApp {
    pub tmp: TempDir,
    pub _app: tauri::App<MockRuntime>,
    pub webview: tauri::WebviewWindow<MockRuntime>,
}

// Drop order (reverse of declaration): webview first, _app second, tmp last.
// This ensures the SQLite connection closes before TempDir deletes the directory.

impl TestApp {
    pub fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let app = make_test_app(tmp.path());
        let webview = WebviewWindowBuilder::new(
            &app,
            "main",
            WebviewUrl::App("index.html".into()),
        )
        .build()
        .expect("build test webview");
        TestApp { tmp, _app: app, webview }
    }

    /// Invoke a Tauri command and unwrap the result. Panics if the command returns an error.
    pub fn invoke<T: DeserializeOwned>(&self, cmd: &str, params: Value) -> T {
        let result = tauri::test::get_ipc_response(
            &self.webview,
            tauri::webview::InvokeRequest {
                cmd: cmd.to_string(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(params),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        );
        match result {
            Ok(body) => body.deserialize::<T>()
                .unwrap_or_else(|e| panic!("invoke '{cmd}' deserialize failed: {e}")),
            Err(e) => panic!("invoke '{cmd}' failed: {e}"),
        }
    }

    /// Open a direct rusqlite connection to the test database for assertions.
    pub fn open_db(&self) -> rusqlite::Connection {
        let db_path = self.tmp.path().join("brain.db");
        let conn = rusqlite::Connection::open(&db_path).expect("open test db for assertions");
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn
    }
}
