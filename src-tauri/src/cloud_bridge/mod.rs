//! Direct desktop-to-cloud vault retrieval bridge. See
//! `docs/superpowers/specs/2026-07-01-clanker-cloud-bridge-design.md`.

pub mod backoff;
pub mod pairing;
pub mod protocol;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::tool_dispatch::{self, ToolDispatchContext};
use protocol::{
    classify_dispatch_error, IncomingFrame, OutgoingMessage, TaskErrorBody, TaskErrorCode,
};

pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);
pub const DEAD_CONNECTION_TIMEOUT: Duration = Duration::from_secs(45);
pub const TOOL_CALL_TIMEOUT: Duration = Duration::from_secs(10);
pub const BACKOFF_BASE: Duration = Duration::from_secs(1);
pub const BACKOFF_CAP: Duration = Duration::from_secs(30);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const AUTH_READY_TIMEOUT: Duration = Duration::from_secs(10);
pub const AUTH_REJECT_RETRY: Duration = Duration::from_secs(300);
pub const WS_CLOSE_AUTH_REJECT: u16 = 4001;
const POLL_TICK: Duration = Duration::from_millis(500);

/// Rejects insecure WebSocket URLs before opening an outbound WebSocket that will carry a
/// pairing token in the first frame.
pub fn validate_ws_url(ws_url: &str) -> anyhow::Result<()> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let request = ws_url.into_client_request()?;
    match request.uri().scheme_str() {
        Some("wss") => Ok(()),
        Some("ws") => {
            let host = request.uri().host().unwrap_or("");
            if host == "localhost" || host == "127.0.0.1" {
                Ok(())
            } else {
                anyhow::bail!(
                    "CURATED_CLANKER_WS_URL must use wss:// (ws:// is only allowed for localhost)"
                );
            }
        }
        _ => anyhow::bail!("CURATED_CLANKER_WS_URL must use wss://"),
    }
}

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
        if validate_ws_url(trimmed).is_err() {
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
    Authenticating,
    Connected,
    Reconnecting,
    AuthRejected,
}

/// Inbound frame from the transport. `Keepalive` covers WebSocket Ping/Pong so
/// `run_session` can refresh liveness without treating them as task payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecvEvent {
    Text(String),
    Keepalive,
    Closed { code: Option<u16> },
}

/// Why `run_session` returned — drives reconnect backoff selection in `run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    Normal,
    AuthRejected,
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
    ) -> impl std::future::Future<Output = anyhow::Result<Option<RecvEvent>>> + Send;
}

fn set_status(status: &Arc<Mutex<ConnectionStatus>>, value: ConnectionStatus) {
    *status.lock().unwrap() = value;
}

/// Interruptible sleep — checks `cancel` every 250ms so `stop()` doesn't have to wait out a
/// full 30s backoff. Mirrors `OutboxWorker::wait_for_cancel`.
async fn interruptible_sleep(
    duration: Duration,
    cancel: &Arc<AtomicBool>,
    retry_now: &Arc<AtomicBool>,
) {
    let chunk = Duration::from_millis(250);
    let mut remaining = duration;
    while remaining > Duration::ZERO {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        if retry_now.swap(false, Ordering::SeqCst) {
            return;
        }
        let next = std::cmp::min(chunk, remaining);
        tokio::time::sleep(next).await;
        remaining = remaining.saturating_sub(next);
    }
}

async fn handle_incoming<T: Transport>(
    ctx: &Arc<ToolDispatchContext>,
    transport: &mut T,
    frame: IncomingFrame,
) -> anyhow::Result<()> {
    let IncomingFrame::Task {
        task_id,
        tool,
        params,
    } = frame
    else {
        return Ok(());
    };

    let response = match tokio::time::timeout(
        TOOL_CALL_TIMEOUT,
        tool_dispatch::dispatch_tool_call(ctx, &tool, params),
    )
    .await
    {
        Ok(Ok(result)) => OutgoingMessage::TaskResult { task_id, result },
        Ok(Err(e)) => OutgoingMessage::TaskError {
            task_id,
            error: TaskErrorBody {
                code: classify_dispatch_error(&e),
                message: e.to_string(),
            },
        },
        Err(_) => OutgoingMessage::TaskError {
            task_id,
            error: TaskErrorBody {
                code: TaskErrorCode::ToolTimeout,
                message: "tool call timed out after 10s".into(),
            },
        },
    };
    transport.send(response).await
}

/// One connected session: auth handshake, heartbeats after `ready`, dispatches inbound tasks,
/// and returns when the transport closes/errors or liveness checks fail.
pub async fn run_session<T: Transport>(
    ctx: &Arc<ToolDispatchContext>,
    mut transport: T,
    pairing_token: &str,
    cancel: &Arc<AtomicBool>,
    status: &Arc<Mutex<ConnectionStatus>>,
) -> SessionEnd {
    if transport
        .send(OutgoingMessage::Auth {
            pairing_token: pairing_token.to_string(),
        })
        .await
        .is_err()
    {
        return SessionEnd::Normal;
    }
    set_status(status, ConnectionStatus::Authenticating);

    let auth_deadline = tokio::time::Instant::now() + AUTH_READY_TIMEOUT;
    let mut authenticated = false;
    let mut last_activity = tokio::time::Instant::now();
    let mut next_heartbeat = tokio::time::Instant::now() + HEARTBEAT_INTERVAL;

    loop {
        if cancel.load(Ordering::SeqCst) {
            return SessionEnd::Normal;
        }

        if !authenticated {
            if tokio::time::Instant::now() >= auth_deadline {
                return SessionEnd::Normal;
            }
        } else {
            if tokio::time::Instant::now() >= next_heartbeat {
                if transport.send(OutgoingMessage::Ping).await.is_err() {
                    return SessionEnd::Normal;
                }
                next_heartbeat = tokio::time::Instant::now() + HEARTBEAT_INTERVAL;
            }
            if last_activity.elapsed() >= DEAD_CONNECTION_TIMEOUT {
                return SessionEnd::Normal;
            }
        }

        let tick = tokio::time::sleep(POLL_TICK);
        tokio::select! {
            recv_result = transport.recv() => {
                match recv_result {
                    Ok(Some(RecvEvent::Text(raw))) => {
                        match serde_json::from_str::<IncomingFrame>(&raw) {
                            Ok(frame) => {
                                last_activity = tokio::time::Instant::now();
                                match frame {
                                    IncomingFrame::Ready if !authenticated => {
                                        authenticated = true;
                                        set_status(status, ConnectionStatus::Connected);
                                        next_heartbeat =
                                            tokio::time::Instant::now() + HEARTBEAT_INTERVAL;
                                    }
                                    IncomingFrame::Pong if authenticated => {}
                                    IncomingFrame::Task { .. } if authenticated => {
                                        if handle_incoming(ctx, &mut transport, frame)
                                            .await
                                            .is_err()
                                        {
                                            return SessionEnd::Normal;
                                        }
                                    }
                                    _ => {
                                        // task before ready, or unknown variant — drop
                                    }
                                }
                            }
                            Err(_) => {
                                // malformed frame — drop
                            }
                        }
                    }
                    Ok(Some(RecvEvent::Keepalive)) => {
                        last_activity = tokio::time::Instant::now();
                    }
                    Ok(Some(RecvEvent::Closed { code })) => {
                        if code == Some(WS_CLOSE_AUTH_REJECT) {
                            return SessionEnd::AuthRejected;
                        }
                        return SessionEnd::Normal;
                    }
                    Ok(None) | Err(_) => return SessionEnd::Normal,
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
    pairing_token: String,
    status: Arc<Mutex<ConnectionStatus>>,
    cancel: Arc<AtomicBool>,
    retry_now: Arc<AtomicBool>,
    mut connect: F,
) where
    T: Transport,
    F: FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<T>> + Send>>,
{
    let mut attempt: u32 = 0;
    let mut auth_rejected = false;

    while !cancel.load(Ordering::SeqCst) {
        set_status(
            &status,
            if auth_rejected {
                ConnectionStatus::AuthRejected
            } else {
                ConnectionStatus::Connecting
            },
        );

        match connect().await {
            Ok(transport) => {
                attempt = 0;
                auth_rejected = false;
                let end = run_session(&ctx, transport, &pairing_token, &cancel, &status).await;

                if cancel.load(Ordering::SeqCst) {
                    break;
                }

                match end {
                    SessionEnd::AuthRejected => {
                        auth_rejected = true;
                        set_status(&status, ConnectionStatus::AuthRejected);
                    }
                    SessionEnd::Normal => {
                        set_status(&status, ConnectionStatus::Reconnecting);
                    }
                }
            }
            Err(_) => {
                set_status(
                    &status,
                    if auth_rejected {
                        ConnectionStatus::AuthRejected
                    } else {
                        ConnectionStatus::Reconnecting
                    },
                );
            }
        }

        if cancel.load(Ordering::SeqCst) {
            break;
        }

        let delay = if auth_rejected {
            AUTH_REJECT_RETRY
        } else {
            backoff::next_delay(attempt, BACKOFF_BASE, BACKOFF_CAP, rand::random())
        };
        if !auth_rejected {
            attempt = attempt.saturating_add(1);
        }
        interruptible_sleep(delay, &cancel, &retry_now).await;
    }
    set_status(&status, ConnectionStatus::Disconnected);
}

/// Real transport over `tokio-tungstenite`. The pairing token is sent as the first text
/// frame after connect (design spec §4).
pub struct WsTransport {
    stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl WsTransport {
    /// Opens a WebSocket with no auth material on the HTTP upgrade. The pairing token is
    /// sent as the first text frame after connect (design spec §4).
    pub async fn connect(ws_url: &str) -> anyhow::Result<Self> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        validate_ws_url(ws_url)?;
        let request = ws_url.into_client_request()?;
        let (stream, _response) =
            tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(request))
                .await??;
        Ok(Self { stream })
    }
}

impl Transport for WsTransport {
    async fn send(&mut self, msg: OutgoingMessage) -> anyhow::Result<()> {
        use futures_util::SinkExt;
        let text = serde_json::to_string(&msg)?;
        self.stream
            .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
            .await?;
        Ok(())
    }

    async fn recv(&mut self) -> anyhow::Result<Option<RecvEvent>> {
        use futures_util::{SinkExt, StreamExt};
        loop {
            match self.stream.next().await {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) => {
                    return Ok(Some(RecvEvent::Text(t.to_string())));
                }
                Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(data))) => {
                    self.stream
                        .send(tokio_tungstenite::tungstenite::Message::Pong(data))
                        .await?;
                    return Ok(Some(RecvEvent::Keepalive));
                }
                Some(Ok(tokio_tungstenite::tungstenite::Message::Pong(_))) => {
                    return Ok(Some(RecvEvent::Keepalive));
                }
                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(frame))) => {
                    return Ok(Some(RecvEvent::Closed {
                        code: frame.map(|f| f.code.into()),
                    }));
                }
                None => return Ok(None),
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(e.into()),
            }
        }
    }
}

pub struct CloudBridgeHandle {
    cancel: Arc<AtomicBool>,
    retry_now: Arc<AtomicBool>,
    status: Arc<Mutex<ConnectionStatus>>,
    join: tauri::async_runtime::JoinHandle<()>,
}

impl CloudBridgeHandle {
    pub fn status(&self) -> ConnectionStatus {
        *self.status.lock().unwrap()
    }

    pub fn retry_now(&self) {
        self.retry_now.store(true, Ordering::SeqCst);
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
    let retry_now = Arc::new(AtomicBool::new(false));
    let status = Arc::new(Mutex::new(ConnectionStatus::Disconnected));
    let cancel_for_run = cancel.clone();
    let retry_now_for_run = retry_now.clone();
    let status_for_run = status.clone();

    let join = tauri::async_runtime::spawn(async move {
        run(
            ctx,
            pairing_token,
            status_for_run,
            cancel_for_run,
            retry_now_for_run,
            move || {
                let ws_url = config.ws_url.clone();
                Box::pin(async move { WsTransport::connect(&ws_url).await })
            },
        )
        .await;
    });

    CloudBridgeHandle {
        cancel,
        retry_now,
        status,
        join,
    }
}

#[doc(hidden)]
pub mod test_hooks {
    pub use super::{run_session, SessionEnd, AUTH_REJECT_RETRY, WS_CLOSE_AUTH_REJECT};
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

    #[test]
    fn resolve_none_for_insecure_ws_url() {
        with_var(
            "CURATED_CLANKER_WS_URL",
            Some("ws://evil.example/agent/desktop"),
            || {
                assert_eq!(CloudBridgeConfig::resolve(), None);
            },
        );
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use protocol::TaskErrorCode;
    use rusqlite::Connection;
    use tokio::sync::mpsc;

    pub(super) struct MockTransport {
        pub(super) incoming: mpsc::UnboundedReceiver<RecvEvent>,
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
        async fn recv(&mut self) -> anyhow::Result<Option<RecvEvent>> {
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
            client: "clanker-bridge".into(),
        })
    }

    async fn run_session_with_status(
        ctx: Arc<ToolDispatchContext>,
        transport: MockTransport,
        cancel: Arc<AtomicBool>,
        status: Arc<Mutex<ConnectionStatus>>,
    ) -> SessionEnd {
        run_session(&ctx, transport, "test-token", &cancel, &status).await
    }

    #[tokio::test]
    async fn auth_frame_is_first_send_and_ready_gates_connected() {
        let ctx = seeded_ctx();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(ConnectionStatus::Disconnected));

        let transport = MockTransport {
            incoming: in_rx,
            outgoing: out_tx,
            _out_rx: None,
        };
        let session = tokio::spawn(run_session_with_status(
            ctx,
            transport,
            cancel.clone(),
            status.clone(),
        ));

        let first = out_rx.recv().await.expect("auth frame expected");
        assert_eq!(
            first,
            OutgoingMessage::Auth {
                pairing_token: "test-token".into()
            }
        );
        assert_eq!(*status.lock().unwrap(), ConnectionStatus::Authenticating);

        in_tx
            .send(RecvEvent::Text(r#"{"type":"ready"}"#.into()))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(*status.lock().unwrap(), ConnectionStatus::Connected);

        cancel.store(true, Ordering::SeqCst);
        let _ = session.await;
    }

    #[tokio::test]
    async fn dispatches_task_and_correlates_response_by_task_id() {
        let ctx = seeded_ctx();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(ConnectionStatus::Disconnected));

        in_tx
            .send(RecvEvent::Text(r#"{"type":"ready"}"#.into()))
            .unwrap();
        in_tx
            .send(RecvEvent::Text(
                r#"{"type":"task","taskId":"t1","tool":"wiki_get_ontology","params":{"entityId":"tier_fact"}}"#
                    .into(),
            ))
            .unwrap();
        drop(in_tx);

        let transport = MockTransport {
            incoming: in_rx,
            outgoing: out_tx,
            _out_rx: None,
        };
        run_session_with_status(ctx, transport, cancel, status).await;

        let _auth = out_rx.recv().await;
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
    async fn unknown_tool_produces_structured_task_error() {
        let ctx = seeded_ctx();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(ConnectionStatus::Disconnected));

        in_tx
            .send(RecvEvent::Text(r#"{"type":"ready"}"#.into()))
            .unwrap();
        in_tx
            .send(RecvEvent::Text(
                r#"{"type":"task","taskId":"t2","tool":"delete_everything","params":{}}"#.into(),
            ))
            .unwrap();
        drop(in_tx);

        let transport = MockTransport {
            incoming: in_rx,
            outgoing: out_tx,
            _out_rx: None,
        };
        run_session_with_status(ctx, transport, cancel, status).await;

        let _auth = out_rx.recv().await;
        match out_rx.recv().await.expect("expected a response") {
            OutgoingMessage::TaskError { task_id, error } => {
                assert_eq!(task_id, "t2");
                assert_eq!(error.code, TaskErrorCode::UnknownTool);
                assert!(error.message.contains("unknown tool"));
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
        let status = Arc::new(Mutex::new(ConnectionStatus::Disconnected));

        let transport = MockTransport {
            incoming: in_rx,
            outgoing: out_tx,
            _out_rx: None,
        };
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            run_session_with_status(ctx, transport, cancel, status),
        )
        .await;
        assert!(
            result.is_ok(),
            "run_session must return promptly when cancel is already set"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn no_ping_before_ready() {
        let ctx = seeded_ctx();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(ConnectionStatus::Disconnected));

        let transport = MockTransport {
            incoming: in_rx,
            outgoing: out_tx,
            _out_rx: None,
        };
        let session = tokio::spawn(run_session_with_status(
            ctx,
            transport,
            cancel.clone(),
            status,
        ));

        let _auth = out_rx.recv().await;
        // Stay within AUTH_READY_TIMEOUT so the session survives until ready is sent.
        tokio::time::advance(AUTH_READY_TIMEOUT - Duration::from_secs(1)).await;
        assert!(
            out_rx.try_recv().is_err(),
            "ping must not fire before ready"
        );

        in_tx
            .send(RecvEvent::Text(r#"{"type":"ready"}"#.into()))
            .unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(HEARTBEAT_INTERVAL).await;
        tokio::task::yield_now().await;
        assert_eq!(out_rx.recv().await, Some(OutgoingMessage::Ping));

        cancel.store(true, Ordering::SeqCst);
        let _ = session.await;
    }

    #[tokio::test(start_paused = true)]
    async fn pong_refreshes_liveness() {
        let ctx = seeded_ctx();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let (out_tx, _out_rx) = mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(ConnectionStatus::Disconnected));

        in_tx
            .send(RecvEvent::Text(r#"{"type":"ready"}"#.into()))
            .unwrap();

        let transport = MockTransport {
            incoming: in_rx,
            outgoing: out_tx,
            _out_rx: None,
        };
        let session = tokio::spawn(run_session_with_status(
            ctx,
            transport,
            cancel.clone(),
            status,
        ));

        tokio::time::advance(DEAD_CONNECTION_TIMEOUT - Duration::from_secs(5)).await;
        in_tx
            .send(RecvEvent::Text(r#"{"type":"pong"}"#.into()))
            .unwrap();
        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(
            !session.is_finished(),
            "pong should refresh dead-connection clock"
        );

        cancel.store(true, Ordering::SeqCst);
        let _ = session.await;
    }

    #[tokio::test(start_paused = true)]
    async fn reconnects_after_forty_five_seconds_of_total_silence() {
        let ctx = seeded_ctx();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let (out_tx, _out_rx) = mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(ConnectionStatus::Disconnected));

        in_tx
            .send(RecvEvent::Text(r#"{"type":"ready"}"#.into()))
            .unwrap();

        let transport = MockTransport {
            incoming: in_rx,
            outgoing: out_tx,
            _out_rx: None,
        };
        let result = tokio::time::timeout(
            Duration::from_secs(120),
            run_session_with_status(ctx, transport, cancel, status),
        )
        .await;
        assert!(
            result.is_ok(),
            "run_session must self-terminate after 45s of silence"
        );
    }
}

#[cfg(test)]
mod reconnect_loop_tests {
    use super::session_tests::MockTransport;
    use super::*;
    use rusqlite::Connection;
    use tokio::sync::mpsc;

    #[tokio::test(start_paused = true)]
    async fn run_retries_connect_with_backoff_until_it_succeeds() {
        let ctx = Arc::new(ToolDispatchContext {
            conn: Arc::new(Mutex::new(Connection::open_in_memory().unwrap())),
            profile: crate::embedder::EmbedProfile::default(),
            vault_dir: None,
            client: "clanker-bridge".into(),
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let retry_now = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(ConnectionStatus::Disconnected));
        let attempts = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let attempts_for_connect = attempts.clone();
        let cancel_for_run = cancel.clone();
        let retry_now_for_run = retry_now.clone();
        let status_for_run = status.clone();
        let ctx_for_run = ctx.clone();

        let handle = tokio::spawn(async move {
            run(
                ctx_for_run,
                "test-token".into(),
                status_for_run,
                cancel_for_run,
                retry_now_for_run,
                move || {
                    let attempts = attempts_for_connect.clone();
                    Box::pin(async move {
                        let n = attempts.fetch_add(1, Ordering::SeqCst);
                        if n < 2 {
                            anyhow::bail!("connect refused");
                        }
                        let (in_tx, in_rx) = mpsc::unbounded_channel::<RecvEvent>();
                        let keepalive_tx = in_tx.clone();
                        tokio::spawn(async move {
                            let _ =
                                keepalive_tx.send(RecvEvent::Text(r#"{"type":"ready"}"#.into()));
                            loop {
                                tokio::time::sleep(Duration::from_secs(30)).await;
                                if keepalive_tx.send(RecvEvent::Keepalive).is_err() {
                                    break;
                                }
                            }
                        });
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

    #[tokio::test(start_paused = true)]
    async fn close_4001_enters_auth_rejected_and_retries_on_five_minute_interval() {
        let ctx = Arc::new(ToolDispatchContext {
            conn: Arc::new(Mutex::new(Connection::open_in_memory().unwrap())),
            profile: crate::embedder::EmbedProfile::default(),
            vault_dir: None,
            client: "clanker-bridge".into(),
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let retry_now = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(ConnectionStatus::Disconnected));
        let attempts = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let attempts_for_connect = attempts.clone();
        let cancel_for_run = cancel.clone();
        let retry_now_for_run = retry_now.clone();
        let status_for_run = status.clone();

        let handle = tokio::spawn(async move {
            run(
                ctx,
                "bad-token".into(),
                status_for_run,
                cancel_for_run,
                retry_now_for_run,
                move || {
                    let attempts = attempts_for_connect.clone();
                    Box::pin(async move {
                        let n = attempts.fetch_add(1, Ordering::SeqCst);
                        let (in_tx, in_rx) = mpsc::unbounded_channel();
                        if n == 0 {
                            in_tx
                                .send(RecvEvent::Closed {
                                    code: Some(WS_CLOSE_AUTH_REJECT),
                                })
                                .unwrap();
                            drop(in_tx);
                        } else {
                            tokio::spawn(async move {
                                let _ = in_tx.send(RecvEvent::Text(r#"{"type":"ready"}"#.into()));
                            });
                        }
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

        tokio::time::advance(Duration::from_millis(100)).await;
        tokio::task::yield_now().await;
        assert_eq!(*status.lock().unwrap(), ConnectionStatus::AuthRejected);

        tokio::time::advance(AUTH_REJECT_RETRY - Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(attempts.load(Ordering::SeqCst), 1);

        retry_now.store(true, Ordering::SeqCst);
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(200)).await;
        tokio::task::yield_now().await;
        assert!(attempts.load(Ordering::SeqCst) >= 2);

        cancel.store(true, Ordering::SeqCst);
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
    }
}
