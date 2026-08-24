//! Throwaway mock `/agent/desktop` WebSocket server. Exercises `cloud_bridge::WsTransport`
//! against a real socket and a real (in-memory, seeded) brain.db through the actual
//! `run_session` path — not the mock transport used by the unit tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rusqlite::Connection;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

use tauri_app_lib::cloud_bridge::test_hooks::run_session;
use tauri_app_lib::cloud_bridge::{self, ConnectionStatus};
use tauri_app_lib::embedder::EmbedProfile;
use tauri_app_lib::tool_dispatch::ToolDispatchContext;

fn seeded_ctx() -> ToolDispatchContext {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE llm_wiki_entries (id TEXT, entity_id TEXT, title TEXT,
            embedding_blob BLOB, deleted_at INTEGER);
         CREATE TABLE llm_wiki_edges (source_id TEXT, target_id TEXT, edge_type TEXT, entity_id TEXT);
         CREATE TABLE llm_wiki_entity_manifests (
            entity_id TEXT PRIMARY KEY, mode TEXT NOT NULL, manifest_json TEXT NOT NULL, updated_at INTEGER);
         INSERT INTO llm_wiki_entries VALUES ('a', 'tier_fact', 'Entry A', NULL, NULL);",
    )
    .unwrap();
    ToolDispatchContext {
        conn: Arc::new(Mutex::new(conn)),
        profile: EmbedProfile::default(),
        vault_dir: None,
        client: "test-client".into(),
    }
}

#[tokio::test]
async fn wiki_get_ontology_round_trips_through_auth_and_typed_envelopes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();

        let auth = loop {
            match ws.next().await {
                Some(Ok(Message::Text(t))) => break t,
                Some(Ok(_)) => continue,
                other => panic!("expected auth text frame, got {other:?}"),
            }
        };
        let auth_json: serde_json::Value = serde_json::from_str(&auth).unwrap();
        assert_eq!(auth_json["type"], "auth");
        assert_eq!(auth_json["pairingToken"], "test-pairing-token");

        ws.send(Message::Text(r#"{"type":"ready"}"#.into()))
            .await
            .unwrap();
        ws.send(Message::Text(
            r#"{"type":"task","taskId":"t1","tool":"wiki_get_ontology","params":{"entityId":"tier_fact"}}"#.into(),
        ))
        .await
        .unwrap();

        let reply = loop {
            match ws.next().await {
                Some(Ok(Message::Text(t))) => break t,
                Some(Ok(_)) => continue,
                other => panic!("expected a text reply, got {other:?}"),
            }
        };
        let json: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(json["type"], "task_result");
        assert_eq!(json["taskId"], "t1");
        assert_eq!(json["result"]["mode"], "off");

        ws.close(None).await.unwrap();
    });

    let transport = cloud_bridge::WsTransport::connect(&format!("ws://{addr}"))
        .await
        .expect("client should connect");

    let ctx = Arc::new(seeded_ctx());
    let cancel = Arc::new(AtomicBool::new(false));
    let status = Arc::new(Mutex::new(ConnectionStatus::Disconnected));

    let cancel_for_session = cancel.clone();
    let session = tokio::spawn(async move {
        run_session(
            &ctx,
            transport,
            "test-pairing-token",
            &cancel_for_session,
            &status,
        )
        .await
    });

    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    cancel.store(true, Ordering::SeqCst);
    let _ = session.await;
}

#[tokio::test]
async fn unknown_tool_produces_a_typed_task_error_frame() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();

        let auth = loop {
            match ws.next().await {
                Some(Ok(Message::Text(t))) => break t,
                Some(Ok(_)) => continue,
                other => panic!("expected auth text frame, got {other:?}"),
            }
        };
        let auth_json: serde_json::Value = serde_json::from_str(&auth).unwrap();
        assert_eq!(auth_json["type"], "auth");
        assert_eq!(auth_json["pairingToken"], "test-pairing-token");

        ws.send(Message::Text(r#"{"type":"ready"}"#.into()))
            .await
            .unwrap();
        ws.send(Message::Text(
            r#"{"type":"task","taskId":"t2","tool":"delete_everything","params":{}}"#.into(),
        ))
        .await
        .unwrap();

        let reply = loop {
            match ws.next().await {
                Some(Ok(Message::Text(t))) => break t,
                Some(Ok(_)) => continue,
                other => panic!("expected a text reply, got {other:?}"),
            }
        };
        let json: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(json["type"], "task_error");
        assert_eq!(json["taskId"], "t2");
        assert_eq!(json["error"]["code"], "UNKNOWN_TOOL");
        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown tool"));
        assert!(json.get("result").is_none());

        ws.close(None).await.unwrap();
    });

    let transport = cloud_bridge::WsTransport::connect(&format!("ws://{addr}"))
        .await
        .unwrap();
    let ctx = Arc::new(seeded_ctx());
    let cancel = Arc::new(AtomicBool::new(false));
    let status = Arc::new(Mutex::new(ConnectionStatus::Disconnected));

    let cancel_for_session = cancel.clone();
    let session = tokio::spawn(async move {
        run_session(
            &ctx,
            transport,
            "test-pairing-token",
            &cancel_for_session,
            &status,
        )
        .await
    });

    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    cancel.store(true, Ordering::SeqCst);
    let _ = session.await;
}
