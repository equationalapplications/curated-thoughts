//! Direct desktop-to-cloud vault retrieval bridge. See
//! `docs/superpowers/specs/2026-07-01-clanker-cloud-bridge-design.md`.

pub mod backoff;
pub mod pairing;
pub mod protocol;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use protocol::{IncomingTask, OutgoingMessage};
use crate::tool_dispatch::{self, ToolDispatchContext};

pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);
pub const DEAD_CONNECTION_TIMEOUT: Duration = Duration::from_secs(45);
pub const TOOL_CALL_TIMEOUT: Duration = Duration::from_secs(10);
pub const BACKOFF_BASE: Duration = Duration::from_secs(1);
pub const BACKOFF_CAP: Duration = Duration::from_secs(30);
const POLL_TICK: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudBridgeConfig {
    pub ws_url: String,
}

impl CloudBridgeConfig {
    /// Reads `CURATED_CLANKER_WS_URL`. Returns `None` when unset or blank — the bridge stays
    /// inert with no config present, mirroring `retrieval::resolve_brain_paths`' env-driven
    /// resolution and the `OutboxWorker` auto-init pattern (§4 of the design spec).
    pub fn resolve() -> Option<Self> {
        let ws_url = std::env::var("CURATED_CLANKER_WS_URL").ok()?;
        let trimmed = ws_url.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(Self {
            ws_url: trimmed.to_string(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

/// Abstracts the WebSocket connection so the state machine is testable without a real socket.
/// Mirrors `outbox::Sink`.
pub trait Transport: Send + 'static {
    fn send(
        &mut self,
        msg: OutgoingMessage,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;
    /// `Ok(None)` means the connection closed cleanly.
    fn recv(
        &mut self,
    ) -> impl std::future::Future<Output = anyhow::Result<Option<String>>> + Send;
}

fn set_status(status: &Arc<Mutex<ConnectionStatus>>, value: ConnectionStatus) {
    *status.lock().unwrap() = value;
}

/// Interruptible sleep — checks `cancel` every 250ms so `stop()` doesn't have to wait out a
/// full 30s backoff. Mirrors `OutboxWorker::wait_for_cancel`.
async fn interruptible_sleep(duration: Duration, cancel: &Arc<AtomicBool>) {
    let chunk = Duration::from_millis(250);
    let mut remaining = duration;
    while remaining > Duration::ZERO {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        let next = std::cmp::min(chunk, remaining);
        tokio::time::sleep(next).await;
        remaining = remaining.saturating_sub(next);
    }
}

async fn handle_incoming<T: Transport>(ctx: &Arc<ToolDispatchContext>, transport: &mut T, raw: &str) {
    let Ok(task) = serde_json::from_str::<IncomingTask>(raw) else {
        return;
    };
    let response = match tokio::time::timeout(
        TOOL_CALL_TIMEOUT,
        tool_dispatch::dispatch_tool_call(ctx, &task.tool, task.params),
    )
    .await
    {
        Ok(Ok(result)) => OutgoingMessage::TaskResult {
            task_id: task.task_id,
            result,
        },
        Ok(Err(e)) => OutgoingMessage::TaskError {
            task_id: task.task_id,
            error: e.to_string(),
        },
        Err(_) => OutgoingMessage::TaskError {
            task_id: task.task_id,
            error: "tool call timed out after 10s".to_string(),
        },
    };
    let _ = transport.send(response).await;
}

/// One connected session: heartbeats every [`HEARTBEAT_INTERVAL`], dispatches inbound tasks,
/// and returns when the transport closes/errors or [`DEAD_CONNECTION_TIMEOUT`] passes with no
/// inbound activity at all (a defensive client-side liveness check independent of whatever
/// liveness bookkeeping Clanker does server-side — see §4 of the design spec).
async fn run_session<T: Transport>(
    ctx: &Arc<ToolDispatchContext>,
    mut transport: T,
    cancel: &Arc<AtomicBool>,
) {
    let mut last_activity = tokio::time::Instant::now();
    let mut next_heartbeat = tokio::time::Instant::now() + HEARTBEAT_INTERVAL;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        if tokio::time::Instant::now() >= next_heartbeat {
            if transport.send(OutgoingMessage::Ping).await.is_err() {
                return;
            }
            next_heartbeat = tokio::time::Instant::now() + HEARTBEAT_INTERVAL;
        }
        if last_activity.elapsed() >= DEAD_CONNECTION_TIMEOUT {
            return;
        }
        let tick = tokio::time::sleep(POLL_TICK);
        tokio::select! {
            recv_result = transport.recv() => {
                match recv_result {
                    Ok(Some(raw)) => {
                        last_activity = tokio::time::Instant::now();
                        handle_incoming(ctx, &mut transport, &raw).await;
                    }
                    Ok(None) | Err(_) => return,
                }
            }
            _ = tick => {}
        }
    }
}

/// Outer connect/reconnect loop. `connect` is injected so tests can substitute a mock
/// transport factory instead of a real WebSocket (mirrors `OutboxWorker::run`'s `Sink`
/// injection).
async fn run<T, F>(
    ctx: Arc<ToolDispatchContext>,
    status: Arc<Mutex<ConnectionStatus>>,
    cancel: Arc<AtomicBool>,
    mut connect: F,
) where
    T: Transport,
    F: FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<T>> + Send>>,
{
    let mut attempt: u32 = 0;
    while !cancel.load(Ordering::SeqCst) {
        set_status(&status, ConnectionStatus::Connecting);
        match connect().await {
            Ok(transport) => {
                attempt = 0;
                set_status(&status, ConnectionStatus::Connected);
                run_session(&ctx, transport, &cancel).await;
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                set_status(&status, ConnectionStatus::Reconnecting);
            }
            Err(_) => {
                set_status(&status, ConnectionStatus::Reconnecting);
            }
        }
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let delay = backoff::next_delay(attempt, BACKOFF_BASE, BACKOFF_CAP, rand::random());
        attempt = attempt.saturating_add(1);
        interruptible_sleep(delay, &cancel).await;
    }
    set_status(&status, ConnectionStatus::Disconnected);
}

/// Real transport over `tokio-tungstenite`. The pairing token is sent as an `Authorization:
/// Bearer` header on the WS upgrade request (§4: "pairing token in connect handshake").
pub struct WsTransport {
    stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl WsTransport {
    pub async fn connect(ws_url: &str, pairing_token: &str) -> anyhow::Result<Self> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = ws_url.into_client_request()?;
        request
            .headers_mut()
            .insert("Authorization", format!("Bearer {pairing_token}").parse()?);
        let (stream, _response) = tokio_tungstenite::connect_async(request).await?;
        Ok(Self { stream })
    }
}

impl Transport for WsTransport {
    async fn send(&mut self, msg: OutgoingMessage) -> anyhow::Result<()> {
        use futures_util::SinkExt;
        let text = msg.to_json_string()?;
        self.stream
            .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
            .await?;
        Ok(())
    }

    async fn recv(&mut self) -> anyhow::Result<Option<String>> {
        use futures_util::StreamExt;
        loop {
            match self.stream.next().await {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) => {
                    return Ok(Some(t.to_string()));
                }
                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => {
                    return Ok(None);
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(e.into()),
            }
        }
    }
}

pub struct CloudBridgeHandle {
    cancel: Arc<AtomicBool>,
    status: Arc<Mutex<ConnectionStatus>>,
    join: tauri::async_runtime::JoinHandle<()>,
}

impl CloudBridgeHandle {
    pub fn status(&self) -> ConnectionStatus {
        *self.status.lock().unwrap()
    }

    pub async fn stop(self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.join.await;
    }
}

/// Starts the bridge on Tauri's async runtime. Inert unless the caller already confirmed
/// [`CloudBridgeConfig::resolve`] returned `Some` and a pairing token was found.
pub fn spawn(
    config: CloudBridgeConfig,
    pairing_token: String,
    ctx: ToolDispatchContext,
) -> CloudBridgeHandle {
    let ctx = Arc::new(ctx);
    let cancel = Arc::new(AtomicBool::new(false));
    let status = Arc::new(Mutex::new(ConnectionStatus::Disconnected));
    let cancel_for_run = cancel.clone();
    let status_for_run = status.clone();

    let join = tauri::async_runtime::spawn(async move {
        run(ctx, status_for_run, cancel_for_run, move || {
            let ws_url = config.ws_url.clone();
            let token = pairing_token.clone();
            Box::pin(async move { WsTransport::connect(&ws_url, &token).await })
        })
        .await;
    });

    CloudBridgeHandle {
        cancel,
        status,
        join,
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use temp_env::with_var;

    #[test]
    fn resolve_none_when_unset() {
        with_var("CURATED_CLANKER_WS_URL", None::<String>, || {
            assert_eq!(CloudBridgeConfig::resolve(), None);
        });
    }

    #[test]
    fn resolve_none_when_blank() {
        with_var("CURATED_CLANKER_WS_URL", Some("   "), || {
            assert_eq!(CloudBridgeConfig::resolve(), None);
        });
    }

    #[test]
    fn resolve_some_when_set() {
        with_var(
            "CURATED_CLANKER_WS_URL",
            Some("wss://example.test/agent/desktop"),
            || {
                assert_eq!(
                    CloudBridgeConfig::resolve(),
                    Some(CloudBridgeConfig {
                        ws_url: "wss://example.test/agent/desktop".to_string()
                    })
                );
            },
        );
    }

    #[test]
    fn resolve_trims_whitespace() {
        with_var(
            "CURATED_CLANKER_WS_URL",
            Some("  wss://example.test/agent/desktop  "),
            || {
                assert_eq!(
                    CloudBridgeConfig::resolve(),
                    Some(CloudBridgeConfig {
                        ws_url: "wss://example.test/agent/desktop".to_string()
                    })
                );
            },
        );
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use rusqlite::Connection;
    use tokio::sync::mpsc;

    pub(super) struct MockTransport {
        pub(super) incoming: mpsc::UnboundedReceiver<String>,
        pub(super) outgoing: mpsc::UnboundedSender<OutgoingMessage>,
        #[allow(dead_code)]
        pub(super) _out_rx: Option<mpsc::UnboundedReceiver<OutgoingMessage>>,
    }

    impl Transport for MockTransport {
        async fn send(&mut self, msg: OutgoingMessage) -> anyhow::Result<()> {
            self.outgoing
                .send(msg)
                .map_err(|_| anyhow::anyhow!("outgoing channel closed"))
        }
        async fn recv(&mut self) -> anyhow::Result<Option<String>> {
            Ok(self.incoming.recv().await)
        }
    }

    fn seeded_ctx() -> Arc<ToolDispatchContext> {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE llm_wiki_entries (id TEXT, entity_id TEXT, title TEXT, deleted_at INTEGER);
             CREATE TABLE llm_wiki_edges (source_id TEXT, target_id TEXT, edge_type TEXT, entity_id TEXT);
             CREATE TABLE llm_wiki_entity_manifests (
                entity_id TEXT PRIMARY KEY, mode TEXT NOT NULL, manifest_json TEXT NOT NULL, updated_at INTEGER);
             INSERT INTO llm_wiki_entries VALUES ('a', 'tier_fact', 'Entry A', NULL);",
        )
        .unwrap();
        Arc::new(ToolDispatchContext {
            conn: Arc::new(Mutex::new(conn)),
            profile: crate::embedder::EmbedProfile::default(),
            vault_dir: None,
        })
    }

    #[tokio::test]
    async fn dispatches_task_and_correlates_response_by_task_id() {
        let ctx = seeded_ctx();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));

        in_tx
            .send(r#"{"taskId":"t1","tool":"wiki_get_ontology","params":{"entityId":"tier_fact"}}"#.to_string())
            .unwrap();
        drop(in_tx);

        let transport = MockTransport {
            incoming: in_rx,
            outgoing: out_tx,
            _out_rx: None,
        };
        run_session(&ctx, transport, &cancel).await;

        let msg = out_rx.recv().await.expect("expected a response");
        match msg {
            OutgoingMessage::TaskResult { task_id, result } => {
                assert_eq!(task_id, "t1");
                assert_eq!(result["mode"], "off");
            }
            other => panic!("expected TaskResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_tool_produces_task_error_not_a_dropped_response() {
        let ctx = seeded_ctx();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));

        in_tx
            .send(r#"{"taskId":"t2","tool":"delete_everything","params":{}}"#.to_string())
            .unwrap();
        drop(in_tx);

        let transport = MockTransport {
            incoming: in_rx,
            outgoing: out_tx,
            _out_rx: None,
        };
        run_session(&ctx, transport, &cancel).await;

        match out_rx.recv().await.expect("expected a response") {
            OutgoingMessage::TaskError { task_id, error } => {
                assert_eq!(task_id, "t2");
                assert!(error.contains("unknown tool"));
            }
            other => panic!("expected TaskError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancel_flag_stops_the_session_promptly() {
        let ctx = seeded_ctx();
        let (_in_tx, in_rx) = mpsc::unbounded_channel();
        let (out_tx, _out_rx) = mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(true));

        let transport = MockTransport {
            incoming: in_rx,
            outgoing: out_tx,
            _out_rx: None,
        };
        let result =
            tokio::time::timeout(Duration::from_secs(2), run_session(&ctx, transport, &cancel)).await;
        assert!(
            result.is_ok(),
            "run_session must return promptly when cancel is already set"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn sends_heartbeat_after_twenty_seconds_of_silence() {
        let ctx = seeded_ctx();
        let (_in_tx, in_rx) = mpsc::unbounded_channel();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_stop = cancel.clone();

        let transport = MockTransport {
            incoming: in_rx,
            outgoing: out_tx,
            _out_rx: None,
        };
        let session = tokio::spawn(async move { run_session(&ctx, transport, &cancel).await });

        let ping = tokio::time::timeout(Duration::from_secs(60), out_rx.recv())
            .await
            .expect("heartbeat did not fire within the timeout")
            .expect("channel closed before a heartbeat was sent");
        assert_eq!(ping, OutgoingMessage::Ping);

        cancel_for_stop.store(true, Ordering::SeqCst);
        let _ = tokio::time::timeout(Duration::from_secs(5), session).await;
    }

    #[tokio::test(start_paused = true)]
    async fn reconnects_after_forty_five_seconds_of_total_silence() {
        let ctx = seeded_ctx();
        let (_in_tx, in_rx) = mpsc::unbounded_channel();
        let (out_tx, _out_rx) = mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));

        let transport = MockTransport {
            incoming: in_rx,
            outgoing: out_tx,
            _out_rx: None,
        };
        let result =
            tokio::time::timeout(Duration::from_secs(120), run_session(&ctx, transport, &cancel))
                .await;
        assert!(
            result.is_ok(),
            "run_session must self-terminate after 45s of silence"
        );
    }
}

#[cfg(test)]
mod reconnect_loop_tests {
    use super::*;
    use rusqlite::Connection;
    use tokio::sync::mpsc;
    use super::session_tests::MockTransport;

    #[tokio::test(start_paused = true)]
    async fn run_retries_connect_with_backoff_until_it_succeeds() {
        let ctx = Arc::new(ToolDispatchContext {
            conn: Arc::new(Mutex::new(Connection::open_in_memory().unwrap())),
            profile: crate::embedder::EmbedProfile::default(),
            vault_dir: None,
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(ConnectionStatus::Disconnected));
        let attempts = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let attempts_for_connect = attempts.clone();
        let cancel_for_run = cancel.clone();
        let status_for_run = status.clone();
        let ctx_for_run = ctx.clone();

        let handle = tokio::spawn(async move {
            run(
                ctx_for_run,
                status_for_run,
                cancel_for_run,
                move || {
                    let attempts = attempts_for_connect.clone();
                    Box::pin(async move {
                        let n = attempts.fetch_add(1, Ordering::SeqCst);
                        if n < 2 {
                            anyhow::bail!("connect refused");
                        }
                        let (in_tx, in_rx) = mpsc::unbounded_channel::<String>();
                        let keepalive_tx = in_tx.clone();
                        tokio::spawn(async move {
                            loop {
                                tokio::time::sleep(Duration::from_secs(30)).await;
                                if keepalive_tx.send("keepalive".to_string()).is_err() {
                                    break;
                                }
                            }
                        });
                        drop(in_tx);
                        let (out_tx, out_rx) = mpsc::unbounded_channel();
                        Ok(MockTransport {
                            incoming: in_rx,
                            outgoing: out_tx,
                            _out_rx: Some(out_rx),
                        })
                    })
                },
            )
            .await;
        });

        tokio::time::sleep(Duration::from_secs(35)).await;
        assert!(
            attempts.load(Ordering::SeqCst) >= 3,
            "expected at least 3 connect attempts"
        );
        assert_eq!(*status.lock().unwrap(), ConnectionStatus::Connected);

        cancel.store(true, Ordering::SeqCst);
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
    }
}
