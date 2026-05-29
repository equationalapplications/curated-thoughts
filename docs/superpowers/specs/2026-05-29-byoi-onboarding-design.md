# BYOI Onboarding + Provider Routing — Design Spec

**Date:** 2026-05-29
**Status:** Approved — ready for implementation planning

---

## Problem

Curated Thoughts currently requires Ollama to be installed and running for both embeddings and text generation. There is no way to skip the Ollama setup step, no support for external inference providers, and no graceful degradation when generation is unavailable. Users who already run LM Studio, a remote OpenAI-compatible endpoint, or their own Ollama instance at a non-default URL have no path to configure that.

---

## Goals

1. Onboarding completes without requiring any model download (skip path is a first-class option).
2. Document indexing (embedding + chunking) works without Ollama for all users.
3. Text generation routes to one of two backends: a local llama-server sidecar, or any OpenAI-compatible external URL.
4. Frontend never knows which backend is active — single Tauri command, single call signature.
5. Prompt formatting is handled server-side for all backends (no client-side chat templates).
6. Config lives in `.brain/config.json`, separate from relational data in `brain.db`.

---

## Architecture Overview

```
Frontend (React)
  invoke("generate_text", { systemPrompt, userPrompt })
  invoke("embed_text", { text })          ← unchanged
        │
        ▼
Rust AppState
  GenerationProvider (Mutex<GenerationProvider>)
    variant Sidecar   { port: u16, child: tokio::process::Child }
    variant External  { base_url: String, api_key: Option<String> }
    variant Unconfigured
  EmbedProvider: fastembed (always local, always in-process)
        │
        ├── Sidecar  → POST http://127.0.0.1:{port}/v1/chat/completions
        └── External → POST {base_url}/v1/chat/completions
```

**Chat template strategy:** Migrate `generate_summary` and `generate_text` from
`/api/generate` (Ollama-specific) to `/v1/chat/completions` with structured
`system`/`user` message objects. All three backends — Ollama, LM Studio,
llama.cpp server — apply the correct model chat template server-side.
The client never handles `<|start_header_id|>`, `<|im_start|>`, or `[INST]`.

---

## Config: `.brain/config.json`

Separate from `brain.db`. Resolved via `CURATED_BRAIN_CONFIG` env var (already
stubbed), or defaults to `<vault>/.brain/config.json`.

```json
{
  "generation": {
    "provider": "sidecar",
    "model_path": "models/llama-3.2-3b.gguf",
    "external_url": null,
    "api_key": null
  },
  "embedding": {
    "provider": "fastembed",
    "external_url": null
  }
}
```

`model_path` is stored **relative to `CURATED_BRAIN_DIR`** (e.g., `models/llama-3.2-3b.gguf`).
`initialize_provider()` joins it with the resolved brain dir to get an absolute path for
`tokio::process::Command`. This survives vault migration across machines.

Writes are **atomic**: write to a temp file in the same directory, then `rename()`.
A write failure during a user-initiated settings save is a **hard failure** — the
in-memory state is rolled back to match the on-disk state, and `Err` is returned
to the frontend. There is no silent ghost-state divergence between memory and disk.

Missing `config.json` on first launch → provider defaults to `Unconfigured`.

---

## New Rust Module: `src-tauri/src/inference/`

```
inference/
  mod.rs       — GenerationProvider enum, generate_text command, update_provider command
  sidecar.rs   — spawn, port selection, await_sidecar_ready, Drop impl
  config.rs    — LlmConfig / GenerationConfig / EmbeddingConfig structs, read/write
```

### `GenerationProvider` enum

```rust
pub enum GenerationProvider {
    Sidecar {
        port: u16,
        child: tokio::process::Child,
    },
    External {
        base_url: String,
        api_key: Option<String>,
    },
    Unconfigured,
}
```

`Drop` on `Sidecar` variant calls `child.kill()`. Orphan prevention is automatic
when state is swapped or the app exits.

### Port selection

Use `portpicker::pick_unused_port()` immediately before spawning the child process.
Do not hardcode a port — collisions with developer tools are common.

### `await_sidecar_ready`

Poll `GET http://127.0.0.1:{port}/health` at 500ms intervals.
The llama.cpp server returns `{"status":"loading model"}` during init and
`{"status":"ok"}` when ready.

Each iteration: check `child.try_wait()` — if the child has exited, the process
crashed (likely OOM). Abort immediately, do not wait for the timeout.

Timeout: 120 seconds. Covers large models on slow NVMe.

Emit `provider-loading` Tauri event each iteration so the frontend can display
"Waking up the Librarian…" with elapsed seconds.

```rust
async fn await_sidecar_ready(
    port: u16,
    child: &mut tokio::process::Child,
    app: &AppHandle,
) -> Result<()> {
    let client = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    let url = format!("http://127.0.0.1:{port}/health");
    loop {
        if Instant::now() > deadline {
            return Err(anyhow!("sidecar startup timed out after 120s"));
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(anyhow!("sidecar exited during startup ({})", status));
        }
        if let Ok(r) = client.get(&url).send().await {
            if let Ok(body) = r.json::<serde_json::Value>().await {
                if body["status"] == "ok" {
                    return Ok(());
                }
            }
        }
        let elapsed = Instant::now().duration_since(deadline - Duration::from_secs(120));
        let _ = app.emit("provider-loading", json!({ "elapsed_s": elapsed.as_secs() }));
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
```

### `update_provider` (transactional swap)

```
lock state
  kill old child if Sidecar
  attempt initialize_provider(new_config)
    success → update state in place, write config.json atomically, return Ok
    failure → state = Unconfigured, write Unconfigured to config.json, return Err
```

If both the new provider init and the config write fail, state is `Unconfigured`
and the frontend is told. The app is never in an ambiguous on-disk vs in-memory state.

### `/v1/chat/completions` payload

```rust
#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
}
```

Serializes to:
```json
{
  "model": "...",
  "messages": [
    { "role": "system", "content": "..." },
    { "role": "user",   "content": "..." }
  ]
}
```

`model` field: sidecar path uses the GGUF filename; external providers use a
configurable model name stored alongside `external_url` in `config.json`.

---

## Modified: Embeddings

`embed_batch(EmbedProfile::Local)` switches from `OllamaEmbedder` to the
existing in-process `Embedder` (fastembed / MiniLM-L6-V2, already in codebase
for benchmarks). `OllamaEmbedder` is kept for `EmbedProfile::Cloud` (escape hatch).

fastembed downloads its model (~90MB) on first `TextEmbedding::try_new()` call.
This is triggered during onboarding (`StepFastembed`) or, for returning users,
during the app loading screen before `AppShell` renders — both paths emit
`embed-init-progress` for an indeterminate spinner.

On success: emit `embed-init-done`. On failure: emit `embed-init-error` with message.
If `fastembed` init fails, `embed_text` returns an error that is caught by the
existing `onRetrievalFallback` in `wiki.ts`, which degrades to keyword-only search.

---

## Boot Sequence

The Tauri `setup` hook must return immediately so the React window renders.
Heavy initialization runs in a detached `tokio::task`.

```
Tauri setup hook:
  → load_config() → defaults if missing
  → register AppState with GenerationProvider::Unconfigured (or loaded value)
  → spawn detached tokio::task:
      → fastembed TextEmbedding::try_new() [emits embed-init-progress]
      → if config = sidecar:
          → portpicker, spawn child, await_sidecar_ready [emits provider-loading]
          → on success: update state to Sidecar, emit provider-ready
          → on failure: update state to Unconfigured, emit provider-error
      → if config = external: emit provider-ready (no startup work needed)
      → if config = unconfigured: emit provider-ready (frontend handles empty state)
  → return Ok(()) ← React window renders now

React:
  → listens for embed-init-progress → indeterminate spinner in loading screen
  → listens for embed-init-done     → spinner clears, loading screen advances
  → listens for embed-init-error    → error banner with retry CTA
  → listens for provider-loading    → "Waking up the Librarian…" spinner in AppShell
  → listens for provider-ready      → spinner clears
  → listens for provider-error      → surfaces actionable error with Settings CTA
```

---

## SetupWizard Changes

Step order: `0=Welcome → 1=StepFastembed → 2=StepModel → 3=StepDone`

### StepFastembed (new)

Invokes a Tauri command that triggers `fastembed` init (or returns immediately
if already initialized). Shows "Setting up local search engine…" with an
indeterminate spinner. Advances automatically on completion.

### StepModel (replaces StepOllama)

Two sub-paths presented as a binary choice:

**Auto-Install path:**
1. Rust detects OS + architecture.
2. Download `llama-server` binary from pinned GitHub release to `.brain/bin/llama-server`.
3. `chmod +x` (macOS/Linux). Windows executable has no chmod step.
4. Verify SHA-256 of binary.
5. Stream GGUF weights to `.brain/models/<filename>` — emit `gguf-download-progress` events.
6. Verify SHA-256 of weights.
7. Write `config.json`: `provider="sidecar"`, `model_path="models/<filename>"` (relative).
8. `initialize_provider()` → spawn sidecar, poll `/health`.
9. Emit `provider-ready`. Advance to StepDone.

Download progress uses the same event + React pattern as the existing Ollama
`pullModel` / `onPullProgress` infrastructure.

**Skip / Use my own path:**
1. Optional: user enters base URL + API key.
2. Write `config.json`: `provider="external"` or `provider="unconfigured"` if blank.
3. Set state. Advance to StepDone.

Note: fastembed runs regardless of which path the user takes. Indexing always works.

---

## Settings UI Changes

`ModelPanel.tsx` splits into two panels:

**GenerationPanel**
- Shows active provider type (sidecar port / external URL / unconfigured).
- If sidecar: shows `provider-loading` / `provider-ready` status, model filename.
- If external: base URL + API key fields. Save calls `update_provider`.
- If unconfigured: "Librarian needs a brain" with Auto-Install and manual config options.
- Save failure (hard failure from Rust) → red toast: "Failed to save settings to disk."

**EmbeddingPanel**
- fastembed shown as default (read-only label).
- Optional external override: URL field for power users routing to OpenAI embeddings or remote Ollama.

---

## Frontend: Error State Mapping

| Rust error | Frontend behavior |
|---|---|
| `ProviderNotReady` | "Waking up the Librarian…" spinner — not shown as error |
| `ProviderUnconfigured` | "Librarian needs a brain" empty state + Settings CTA |
| `GenerationError(msg)` | Toast with message + retry button |
| `ProviderError` (boot/swap failure) | Banner with message + Settings CTA |

`generate_text` returns `ProviderNotReady` while the background task is still
polling `/health`. The frontend must not surface this as a user-visible error —
it maps to the same spinner the `provider-loading` event drives.

---

## `wiki.ts` Change

```ts
// Before
return invoke<string>("ollama_generate", { systemPrompt, userPrompt });

// After
return invoke<string>("generate_text", { systemPrompt, userPrompt });
```

No structural change. The wiki package's `generateText` interface is unchanged.

---

## Testing

### Rust unit tests (`src-tauri/tests/`)

**`config.rs`:**
- Round-trip serialize/deserialize `LlmConfig` (all variants).
- Relative `model_path` joined with `CURATED_BRAIN_DIR` produces correct absolute path.
- Atomic write: simulate mid-write crash (delete temp file after write, before rename) — original config survives.

**`inference/mod.rs`:**
- `generate_text` with `Sidecar` variant routes to correct port.
- `generate_text` with `External` variant includes `Authorization` header when `api_key` is set.
- `generate_text` with `Unconfigured` returns `ProviderNotReady`.
- `/v1/chat/completions` payload serializes exactly to expected JSON structure:
  ```json
  {
    "messages": [
      { "role": "system", "content": "system text" },
      { "role": "user",   "content": "user text" }
    ]
  }
  ```
  (Guards against key-name regressions that llama-server silently rejects as 400.)

**`sidecar.rs`:**
- `await_sidecar_ready` returns `Err` when child exits mid-poll (OOM simulation).
- `await_sidecar_ready` returns `Err` after 120s with mock server that never returns `ok`.

**`update_provider` transactional swap:**
- On `initialize_provider` failure: state rolls back to `Unconfigured`, config.json reflects `Unconfigured`.

### Frontend tests (Vitest, `src/__tests__/`)

**`StepModel.test.tsx`:**
- Renders idle → downloading → checksumming → ready → error phases.
- Skip path writes `provider="unconfigured"` when URL field is blank.
- Retry re-invokes download command.

**`GenerationPanel.test.tsx`:**
- Switch from sidecar to external triggers `update_provider` with correct payload.
- `ProviderNotReady` error maps to spinner, not error toast.
- Settings save failure shows red toast.

**`useSetupStatus` hook:**
- `needsSetup` is true when `config.json` is missing.
- `needsSetup` is true when `config.generation.provider === "unconfigured"`.

---

## Out of Scope

- Hot-swap provider without app restart (`tokio::sync::watch` approach — defer to v2).
- Cloud embedding providers (OpenAI, Voyage, Cohere) — `EmbedProfile::Cloud` stub exists, implement separately.
- llama.cpp sidecar binary auto-update — manual re-download or app update handles this.
- Per-folder generation provider override (existing `provider_override` column in `folder_rules` — orthogonal feature).
