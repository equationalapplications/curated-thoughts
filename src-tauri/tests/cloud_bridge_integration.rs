//! Throwaway mock `/agent/desktop` WebSocket server. Exercises `cloud_bridge::WsTransport`
//! against a real socket and a real (in-memory, seeded) brain.db through the actual
//! `tool_dispatch::dispatch_tool_call` path — not the mock transport used by the unit tests.

use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use rusqlite::Connection;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

use tauri_app_lib::cloud_bridge::{self, protocol::OutgoingMessage, Transport};
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
    }
}

#[tokio::test]
async fn wiki_get_ontology_round_trips_over_a_real_websocket() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        ws.send(Message::Text(
            r#"{"taskId":"t1","tool":"wiki_get_ontology","params":{"entityId":"tier_fact"}}"#.into(),
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
        assert_eq!(json["taskId"], "t1");
        assert_eq!(json["result"]["mode"], "off");
        assert!(json.get("error").is_none());

        ws.close(None).await.unwrap();
    });

    let mut transport = cloud_bridge::WsTransport::connect(&format!("ws://{addr}"), "test-pairing-token")
        .await
        .expect("client should connect to the mock server");

    let ctx = Arc::new(seeded_ctx());
    let raw = transport.recv().await.unwrap().expect("expected the task frame");
    let task: cloud_bridge::protocol::IncomingTask = serde_json::from_str(&raw).unwrap();
    assert_eq!(task.task_id, "t1");
    assert_eq!(task.tool, "wiki_get_ontology");

    let result = tauri_app_lib::tool_dispatch::dispatch_tool_call(&ctx, &task.tool, task.params)
        .await
        .unwrap();
    transport
        .send(OutgoingMessage::TaskResult {
            task_id: task.task_id,
            result,
        })
        .await
        .unwrap();

    server.await.unwrap();
}

#[tokio::test]
async fn unknown_tool_produces_a_task_error_frame() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        ws.send(Message::Text(
            r#"{"taskId":"t2","tool":"delete_everything","params":{}}"#.into(),
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
        assert_eq!(json["taskId"], "t2");
        assert!(json["error"].as_str().unwrap().contains("unknown tool"));
        assert!(json.get("result").is_none());

        ws.close(None).await.unwrap();
    });

    let mut transport = cloud_bridge::WsTransport::connect(&format!("ws://{addr}"), "test-pairing-token")
        .await
        .unwrap();
    let ctx = Arc::new(seeded_ctx());
    let raw = transport.recv().await.unwrap().unwrap();
    let task: cloud_bridge::protocol::IncomingTask = serde_json::from_str(&raw).unwrap();

    let response = match tauri_app_lib::tool_dispatch::dispatch_tool_call(&ctx, &task.tool, task.params).await
    {
        Ok(result) => OutgoingMessage::TaskResult {
            task_id: task.task_id,
            result,
        },
        Err(e) => OutgoingMessage::TaskError {
            task_id: task.task_id,
            error: e.to_string(),
        },
    };
    transport.send(response).await.unwrap();

    server.await.unwrap();
}
