# Clanker Desktop Bridge — Wire Alignment (Amendment)

**Date:** 2026-07-05
**Status:** Implemented
**Amends:** `2026-07-01-clanker-cloud-bridge-design.md` (Approved — architecture, trust boundary, and tool contracts unchanged; this spec supersedes only its §4 wire details)
**Fixed external contract:** `clanker/docs/superpowers/specs/2026-07-05-desktop-vault-bridge-design.md` — Clanker's `/agent/desktop` handler spec. Its wire protocol (§5) is treated as authoritative; Curated Thoughts conforms.

## 1. Summary

The Clanker-side spec for `/agent/desktop` pins wire details the original Curated Thoughts spec left as a sketch, and the shipped `CloudBridgeClient` (`src-tauri/src/cloud_bridge/`) now diverges from that contract in four ways. This amendment brings Curated Thoughts into conformance:

1. **Auth handshake** — replace the `Authorization: Bearer` upgrade header with a first-frame `{"type":"auth","pairingToken"}` message, gated on a `{"type":"ready"}` reply.
2. **Typed frame envelopes** — all outgoing frames carry a `type` discriminator (`task_result`, `task_error`); incoming `task`, `pong`, and `ready` frames are parsed explicitly.
3. **Structured errors** — `task_error` carries `error: { code, message }` with a defined CT-side code taxonomy, not a bare string.
4. **Auth-reject handling** — close code `4001` moves the client into a slow-retry state surfaced in Settings, instead of hammering the normal reconnect backoff forever with a dead token.

No new capabilities, no tool-contract changes, no schema changes. The five read-only tools, the in-process dispatch path, keychain token storage, and the 20s-ping / 45s-dead-connection liveness numbers are all unchanged.

## 2. Background

The 2026-07-01 spec described the Clanker side only as a reference contract ("pairing token in connect handshake") and the implementation reasonably chose a Bearer header on the WS upgrade. Clanker's own spec has since committed to a first-frame auth message mirroring its live browser handler (`wsBrowserAgentHandler.ts`), plus zod-validated typed frames. Because the Clanker handler validates every frame against typed schemas and silently drops non-conforming ones, the current CT implementation would fail in two independent ways: the connection would never authenticate (no auth frame → close `4001` after 5s), and even if it did, every `{taskId, result}` reply would be dropped by zod validation.

Decision (owner-confirmed): Curated Thoughts conforms to the Clanker spec as written. Clanker's shape mirrors an already-live handler; bending it toward CT's header auth would fork Clanker's own precedent for no benefit. Neither side is deployed against the other yet, so no dual-protocol compatibility window is needed — this is a clean switch.

## 3. Non-goals

- No change to the five tool contracts (`wiki_search`, `wiki_get_ontology`, `wiki_traverse_graph`, `vault_semantic_search`, `vault_related_chunks`) or their param/result shapes (`2026-06-23-mcp-wiki-graph-tools-design.md`).
- No write path, no review-queue exposure — trust boundary of the 07-01 spec §6 unchanged.
- No pairing-flow changes: token still pasted in Settings, still keychain-stored (`pairing.rs` untouched).
- No changes proposed back to the Clanker spec.
- No dual-protocol / legacy-frame support.

## 4. Handshake and connection lifecycle

`WsTransport::connect` no longer sets an `Authorization` header. The session flow becomes:

```text
connect (wss upgrade, no auth material)
  │
  ▼
send {"type":"auth","pairingToken":"<token>"}   ← first frame, immediately on open
  │
  ▼
wait for {"type":"ready"}                        ← gate; 10s timeout
  │   (Clanker decides within its 5s auth window; 10s adds network headroom)
  │
  ├─ ready received → status Connected, heartbeats start, tasks dispatchable
  ├─ timeout / close (non-4001) → failed connect, normal backoff (1s → 30s)
  └─ close 4001 → status AuthRejected, slow retry (see below)
```

- `ConnectionStatus` gains two variants: `Authenticating` (auth sent, awaiting `ready`) and `AuthRejected`. `Connected` is only set after `ready` — previously it was set as soon as the socket opened, which overstated reality and would have shown "Connected" during a doomed handshake.
- Heartbeat pings start only after `ready`. Nothing but the auth frame is sent before it.
- **Buffered-frame ordering:** the server may send `ready` and the first `task` back-to-back (same TCP segment). The `Authenticating → Connected` transition must happen synchronously inside the same receive loop that dispatches tasks, on the same transport — not as a separate "wait for ready" phase that reads (and could discard) frames before handing the socket to the session loop. A `task` received while still `Authenticating` (i.e. before `ready`) is a protocol violation and is dropped like any unknown frame; a `task` read on the very next loop iteration after `ready` must dispatch normally.
- The pairing token now transits as a WS text frame instead of an HTTP header. Same TLS channel either way (`validate_ws_url` still enforces `wss://` outside localhost), so no security regression; it also stops the token from being eligible for HTTP-layer header logging on any intermediate.

**Close `4001` (auth rejected).** Clanker sends `4001` for an unknown token hash, a revoked device, *and* a paused device — indistinguishable client-side. A paused device is expected to come back, so CT must not stop retrying permanently; a revoked token never recovers, so CT must not stay on the 30s backoff cap indefinitely either. Resolution: on `4001`, enter `AuthRejected` and retry on a fixed slow interval of **5 minutes**, forever, until a connect succeeds (→ normal lifecycle) or the user deletes the token. Unpausing recovers within 5 minutes with no app restart; a revoked token costs Clanker one cheap rejected handshake per 5 minutes until the user re-pairs or clears the token.

Settings surfacing: `CloudBridgePanel` shows the `AuthRejected` state as "Pairing rejected — token revoked or device paused", with a **Retry now** button and the existing remove-token action. Retry-now is a second `AtomicBool` checked by `interruptible_sleep` alongside the existing `cancel` flag: setting it ends the current wait early and clears itself, leaving the rest of the loop untouched.

## 5. Frame envelopes (`protocol.rs`)

All frames are JSON text messages with a `type` discriminator, matching Clanker's zod schemas.

**Incoming (Clanker → CT):**

| Frame | Handling |
|---|---|
| `{"type":"ready"}` | Auth success; transition `Authenticating → Connected` |
| `{"type":"pong"}` | Liveness acknowledgment (reply to our ping) |
| `{"type":"task","taskId","tool","params"}` | Dispatch to `tool_dispatch::dispatch_tool_call` |
| unknown `type` / malformed JSON | Drop, log at debug — mirrors Clanker's post-auth drop-and-log posture |

The 45s dead-connection clock refreshes on **any** well-formed inbound frame (including dropped-unknown ones) plus WS-protocol Ping/Pong, exactly as today — `pong` is now parsed explicitly rather than counting only as opaque text, but the liveness semantics are unchanged.

**Outgoing (CT → Clanker):**

| Frame | Notes |
|---|---|
| `{"type":"auth","pairingToken"}` | First frame only |
| `{"type":"ping"}` | Every 20s after `ready` (unchanged shape/interval) |
| `{"type":"task_result","taskId","result"}` | Was `{taskId, result}` |
| `{"type":"task_error","taskId","error":{"code","message"}}` | Was `{taskId, error: "<string>"}` |

`IncomingTask` becomes a variant of a tagged `IncomingFrame` enum (`#[serde(tag = "type")]`); `OutgoingMessage` gains `Auth` and emits the `type` field on every variant.

## 6. Error-code taxonomy

CT-side codes carried in `task_error.error.code`. Clanker surfaces these to the model through its `TOOL_ERROR` path ("CT's error message, prefixed" — its §8), so `message` must stand alone as human-readable text; `code` is for logging/monitoring stability.

| Code | When |
|---|---|
| `UNKNOWN_TOOL` | `tool` is not one of the five wire names |
| `BAD_PARAMS` | Params fail dispatch-level validation/deserialization |
| `TOOL_TIMEOUT` | Local dispatch exceeded the 10s per-call timeout |
| `INTERNAL` | Any other dispatch error (SQLite failure, embedder failure, …) |

Mapping lives where errors are produced today (`handle_incoming` in `mod.rs`): the timeout arm emits `TOOL_TIMEOUT`; `dispatch_tool_call`'s error is classified into `UNKNOWN_TOOL` / `BAD_PARAMS` / `INTERNAL` (dispatch already distinguishes unknown-tool and param-deserialization failures; they become typed rather than string-matched). `message` keeps the current `e.to_string()` detail.

## 7. Testing plan

Same shapes as the existing suite — no new harness:

- **`protocol.rs` unit tests** — rewrite for the tagged enums: each incoming frame type parses; unknown `type` yields the drop case; every outgoing variant serializes with its `type` field; `task_error` nests `{code, message}`.
- **`run_session` tests (`mod.rs`)** — auth frame is the first send on a new transport; `Connected` only after `ready` (status stays `Authenticating` before it); no ping before `ready`; `pong` refreshes liveness; task dispatch/correlation unchanged against the new envelopes; `4001` close → `AuthRejected` + slow-retry interval (paused-clock tokio test, like the existing backoff tests).
- **Integration test (`cloud_bridge_integration.rs`)** — mock server updated to require the auth frame, reply `ready`, and validate outgoing envelopes; one end-to-end `wiki_search` round trip through the real dispatch path.
- **Manual smoke test** — unchanged from the 07-01 spec §7: pair against Clanker's `docker-compose.local.yml` stack once its `/agent/desktop` handler lands; this amendment is what makes that smoke test able to pass.

## 8. What stays unchanged

- Five tool contracts and the in-process dispatch path (`tool_dispatch`, `wiki_graph.rs`, `retrieval/mod.rs`).
- Keychain pairing-token storage (`pairing.rs`), `CURATED_CLANKER_WS_URL` config resolution, `wss://` enforcement.
- Heartbeat interval (20s), dead-connection timeout (45s), per-call tool timeout (10s — inside Clanker's 12s watcher budget), backoff for ordinary connect failures (1s → 30s + jitter).
- The 07-01 spec's trust boundary (§6) and non-goals (§3) in full.
