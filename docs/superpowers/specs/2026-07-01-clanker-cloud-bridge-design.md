# Clanker Cloud Bridge — Direct Desktop-to-Cloud Vault Retrieval

**Date:** 2026-07-01
**Status:** Draft
**Depends on:** `2026-06-23-mcp-wiki-graph-tools-design.md` (tool contracts reused verbatim), `2026-05-23-unified-mcp-binary-spec.md` (shared `tauri_app_lib`, both binaries), `2026-05-17-prisma-adapter-rust-design.md` (`OutboxWorker` auto-init pattern this design mirrors)
**External repo referenced (contract only, not designed here):** `clanker` (Cloud Coordinator + MV3 extension)

## 1. Summary

This spec adds a direct, persistent connection from Curated Thoughts (the Tauri desktop app) to Clanker's cloud backend, so a Clanker cloud agent can query a user's local knowledge vault (wiki entries, graph edges, semantic chunks) during a chat/voice turn. Curated Thoughts initiates and maintains an outbound WebSocket to Clanker; Clanker dispatches read-only, atomic tool calls over it and gets results back — the same tool contracts already defined for the local MCP server, reused verbatim with zero new protocol.

This is a superseding revision of an earlier draft that routed this integration through the MV3 browser extension (Native Messaging or a localhost server bridging extension → Curated Thoughts). That approach is dropped. See §2 for why and §8 for what remains unchanged on the extension side.

## 2. Background and architectural pivot

Investigation while brainstorming this spec found that Clanker's browser bridge (FCM `WAKE_AND_CONNECT`, Firestore rendezvous, `/agent/browser` WebSocket, Task DSL, destructive-action classifier) is **already built and live** in the `clanker` repo (`docs/browser-bridge.md`), independent of Curated Thoughts — the extension has never talked to Curated Thoughts. The only genuinely new integration surface is a channel for Clanker to reach the local vault.

The original draft proposed reaching that vault through the extension (Native Messaging host or a localhost server the extension calls into). This is rejected for two reasons, per direct product decision:

1. **Chrome Web Store risk isolation.** The extension's CWS review status and the `chrome.gcm` deprecation are risks that should not be able to take down local-vault retrieval. A Native Messaging manifest or localhost server adds exactly the kind of local-code-execution surface CWS review scrutinizes.
2. **Architectural simplicity.** Curated Thoughts talking directly to Clanker over one outbound WebSocket avoids Native Messaging manifests, localhost CORS/auth, and a second hop entirely. Latency floor stays at "local SQLite read," not "local SQLite read plus a browser IPC round trip."

Resulting topology: **two independent spokes off Clanker**, not a triangle.

```text
                    ┌─────────────────────┐
   DOM tasks        │   Clanker Cloud      │      Vault-retrieval tasks
  (unchanged,  ◄────┤   Coordinator        ├────►  (new, this spec)
   out of scope)    └─────────────────────┘
        │                                              │
        ▼                                              ▼
  MV3 Extension                              Curated Thoughts (Tauri)
  (DOM sensor)                               persistent outbound WS
```

## 3. Non-goals

- **Extension transport.** Whatever transport the extension ends up using to reach Clanker (its existing FCM/WebSocket flow, or a future HTTP-only design forced by `chrome.gcm` deprecation or CWS policy) is entirely the `clanker` repo's decision. Not designed here.
- **Durable task queue / offline backlog.** If Curated Thoughts is not connected, a vault-retrieval request fails immediately (no credit spent, per Clanker's existing `getActiveDevice` pattern). No queued delivery on reconnect.
- **Any write path over this channel.** No fact writes, no review-queue approval, no wiki mutation. The channel is read-only, atomic tool calls only. The Librarian's proposal/review-queue loop (`src-tauri/src/librarian/mod.rs`, `get_review_queue`) stays entirely local and human-in-the-loop, unreachable from Clanker.
- **Device-code / QR pairing UX.** v1 is a pasted pairing token only.
- **New wire protocol.** Zero invention — reuses the `wiki_search` / `wiki_get_ontology` / `wiki_traverse_graph` / `vault_semantic_search` / `vault_related_chunks` tool contracts exactly as defined in `2026-06-23-mcp-wiki-graph-tools-design.md`.
- **Clanker-side `/agent/desktop` route implementation.** Described here as a contract Curated Thoughts codes against; the actual Node/Firestore implementation is Clanker's own spec and PR.

## 4. Curated Thoughts components and data flow

New module `src-tauri/src/cloud_bridge/`, added to the shared `tauri_app_lib` crate so both binaries (`curated-thoughts` GUI and `curated-thoughts --mcp` headless) gain it for free — mirrors the `OutboxWorker` auto-init pattern (`src-tauri/src/outbox/mod.rs`): opt-in, inert with no config present, no behavior change for users who don't set it up.

```text
Settings UI: user pastes pairing token (generated in Clanker mobile/web, Settings → Devices)
      │
      ▼
Pairing token stored in OS keychain (`keyring` crate) — never in brain.db, never in a config
file that could be swept up by a routine SQLite backup/export/sync
      │
      ▼
CloudBridgeClient (tokio task, tokio-tungstenite)
      │ connects wss://<clanker-host>/agent/desktop, pairing token in connect handshake
      │ auto-reconnect with exponential backoff + jitter (1s → 30s cap)
      │ app-level heartbeat: sends {"type":"ping"} every 20s (same interval and
      │   message shape as extension/src/background/ws-client.ts, so Clanker's
      │   server-side heartbeat handling is reused as-is for both spokes)
      ▼
On connect: Clanker resolves token → uid + deviceId, marks device online
      (Firestore users/{uid}/devices/{deviceId}, type: "desktop")
      │
      ▼
Clanker sends: { taskId, tool, params }
  tool ∈ { wiki_search, wiki_get_ontology, wiki_traverse_graph,
           vault_semantic_search, vault_related_chunks }
      │
      ▼
CloudBridgeClient dispatches in-process — no subprocess, no stdio, no MCP hop —
directly to the same plain Rust functions the `--mcp` binary's `#[tool]` handlers
already call (src-tauri/src/wiki_graph.rs, src-tauri/src/retrieval/mod.rs).
Those functions are extracted so both the rmcp tool handlers and CloudBridgeClient
call the identical implementation — one code path, two callers.
      │
      ▼
CloudBridgeClient replies: { taskId, result } or { taskId, error }
```

Per-tool-call timeout: 10s (local SQLite/embedding calls are sub-50ms in steady state; 10s covers network RTT plus a cold embedder-model load on the first call of a session, while still failing well inside a single chat turn). Dead-connection detection: no `{"type":"pong"}`-equivalent liveness signal for 45s (~2 missed heartbeats) → Clanker marks the device offline and closes the socket.

No FCM-style wake timeout exists in this design — that concept only makes sense for best-effort async wake. Here, connection state is already known server-side (`lastSeenAt` / `isPaused` on the device doc), so a request either dispatches immediately or fails fast; there is nothing to wait on.

**Agent chaining:** each WebSocket message is one atomic tool call. Clanker's cloud agent (Gemini) is free to chain multiple calls within one turn (e.g. `wiki_search` → `wiki_traverse_graph`), evaluating each result before deciding the next call — mirrored exactly from how `browser_action` already supports chaining (`read_dom` → `click` → `read_dom`). Curated Thoughts does not batch or locally orchestrate a multi-step plan; that reasoning stays in the LLM, keeping this channel's atomic tool contracts unchanged from the local MCP surface. Overall turn budget is bounded by Clanker's existing `MAX_ITERATIONS = 5` agent-loop cap — not duplicated here.

## 5. Clanker-side contract (reference only)

Curated Thoughts codes against this shape; the implementation lives in the `clanker` repo.

- New route `/agent/desktop` (WebSocket), structurally mirroring `cloud-agent/src/handlers/wsBrowserAgentHandler.ts`.
- New device type on the existing `users/{uid}/devices/{deviceId}` collection: `{ type: "desktop", pairingTokenHash, deviceName, lastSeenAt, isPaused }`. Pairing token is generated once in Clanker mobile/web (Settings → Devices), shown once, and stored server-side as a hash (never the raw token).
- New ADK tool, `query_local_vault`, separate from `browser_action`. Resolves the active **desktop**-type device (a `getActiveDevice` variant filtered by `type: "desktop"`); if none connected, returns an error immediately with no credit spent — the same fail-fast contract `browser_action` already uses when no browser device is paired.
- A desktop device belongs to exactly one uid; the pairing token binds 1:1 at generation time. No cross-device fan-out, no shared tokens.

## 6. Trust boundary and security

- Clanker never reads `brain.db` or `documents/` directly — it only ever receives whatever a `wiki_search` / `vault_semantic_search` / etc. call returns (indexed entries/chunks), the same boundary the local MCP tools already enforce for any other MCP client.
- The pairing token grants **query access only**. None of the five exposed tools can write, delete, or approve anything. Write-back (Librarian proposals, review-queue approval) requires local human interaction in the desktop app UI and has no wire path from this channel — enforced by simply not exposing any mutating tool, not by a runtime permission check that could drift.
- Pairing token lives in the OS keychain (`keyring` crate or a Tauri secure-storage plugin), not `brain.db` and not a plaintext config file — isolates the credential from a database file that might otherwise get backed up, synced, or exported as a unit with the user's actual knowledge data.
- Token revocation is Clanker-side (Settings → Devices → remove device), consistent with how the existing browser device pairing is revoked/paused today.

## 7. Testing plan

Mirrors two existing precedents in this codebase and in `clanker` rather than inventing a new testing shape:

- **Unit tests:** `CloudBridgeClient`'s connection state machine (backoff, jitter, 45s dead-connection detection, dispatch/response correlation by `taskId`) is tested against a trait-abstracted transport — same shape as `Sink` in `src-tauri/src/outbox/mod.rs` — so timing and reconnect logic run in milliseconds with no real socket.
- **Local integration tests:** a throwaway `tokio-tungstenite` mock WebSocket server under `src-tauri/tests/`, asserting the wire format (`{taskId, tool, params}` in, `{taskId, result|error}` out) end to end through the real dispatch path into `wiki_graph.rs` / `retrieval/mod.rs`.
- **Manual smoke test:** pair against Clanker's existing local dev stack (`docker-compose.local.yml`, already used for cloud-agent development per `clanker/docs/edge-agent.md`) with a real pairing token; confirm a `wiki_search` round trip end to end. Not run against staging or production.

## 8. What stays unchanged

- The MV3 extension's existing DOM-task capability (`extract`, `summarize_visible_text`, `read_dom`, `open_tab`, `focus_tab`, `scroll`, and the wire-stable-but-not-yet-executed `fill_field`/`click`) is untouched by this spec.
- Curated Thoughts' local MCP server (`--mcp` stdio mode) is untouched and keeps serving Cursor/Claude Desktop/other local MCP clients exactly as before; `CloudBridgeClient` is an additional caller of the same underlying functions, not a replacement transport.
- The Librarian fact-extraction and review-queue pipeline is untouched; it remains local-only regardless of this channel.
