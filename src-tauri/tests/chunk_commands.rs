mod helpers;
use helpers::TestApp;
use serde_json::json;

/// Regression test for the v1.19.0 wire-name bug: the frontend invokes
/// "resolve_chunk_overlay" (src/lib/tauri.ts), and Tauri v2 registers
/// commands under the exact fn ident — so the Rust fn must be named
/// `resolve_chunk_overlay`, not `resolve_chunk_overlay_cmd`.
#[test]
fn resolve_chunk_overlay_is_invocable_by_frontend_name() {
    let app = TestApp::new();
    let resolved: Option<serde_json::Value> =
        app.invoke("resolve_chunk_overlay", json!({ "path": "/nope.md", "hash": "h" }));
    assert_eq!(resolved, None);
}
