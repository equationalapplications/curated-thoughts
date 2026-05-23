# Unified MCP Binary Implementation Spec

**Context**
We are working on the `curated-thoughts` project, a local-first second brain desktop application built with Tauri 2.x, React, and Rust. Currently, the project contains an experimental Model Context Protocol (MCP) server that compiles as a completely separate standalone binary (`curated-thoughts-mcp`) behind a `mcp-server` feature flag.

**Objective**
I want to deprecate the separate MCP binary and integrate the MCP server directly into the main Tauri application binary. The application should act as a multi-call binary. If executed with the `--mcp` command-line flag, it should launch the headless stdio MCP server. If executed normally, it should launch the standard Tauri desktop GUI.

Please implement this architectural change. Below are the specific requirements and steps.

### 1. Refactor `Cargo.toml`

* Remove the separate `[[bin]]` definition for `curated-thoughts-mcp` inside `src-tauri/Cargo.toml`.
* Ensure all necessary dependencies for the MCP server (like `tokio`, MCP protocol crates, etc.) are available to the main library, keeping them gated behind the `mcp-server` feature flag if appropriate, to maintain modularity.

### 2. Update the Main Entry Point (`src-tauri/src/main.rs`)

* Intercept command-line arguments early in `main()` using `std::env::args()`.
* If the `--mcp` flag is detected:
* **Do not** initialize the Tauri Builder, window, or webview.
* Initialize a `tokio::runtime::Runtime` (if not already handled by the library).
* Call the core MCP server event loop (e.g., `curated_thoughts::run_mcp_server_loop()`).
* Exit the process cleanly (`std::process::exit(0)`) when the loop terminates.


* If the flag is missing, proceed with the normal `tauri::Builder::default()...run()` lifecycle.

### 3. Expose the MCP Logic (`src-tauri/src/lib.rs`)

* Ensure the function that starts the stdio MCP server (reading from `stdin` and writing to `stdout`) is publicly exposed from the library crate so `main.rs` can call it.
* Implement proper `#[cfg(feature = "mcp-server")]` attributes to ensure the app still compiles successfully even if the feature flag is disabled (returning an error or exiting if triggered without the compiled feature).

### 4. Enforce Strict `stdout` Hygiene (CRITICAL)

* The MCP protocol relies on strict JSON-RPC communication over `stdout`.
* Ensure that when the app is running in `--mcp` mode, **absolutely no logging, tracing, or debugging output is written to `stdout**`.
* Reroute all `tracing` or `log` events to `stderr` or a local log file when the `--mcp` flag is active to prevent crashing connected AI clients.

### 5. Create Frontend UI Configuration Snippet (React)

* Add a new component or section in the React frontend Settings page called "Developer / Agent Integration".
* Create a read-only code block that dynamically generates the JSON configuration required for an agent (like Cursor or Claude Code) to connect to the embedded server.
* The snippet should dynamically format the absolute path to the binary based on the OS:
* **macOS:** `/Applications/CuratedThoughts.app/Contents/MacOS/curated-thoughts`
* **Windows:** `C:\Program Files\CuratedThoughts\curated-thoughts.exe` (or local AppData path)
* **Linux:** `/usr/bin/curated-thoughts` (or appropriate install path)


* Example snippet to generate for the user:
```json
{
  "mcpServers": {
    "curated-thoughts": {
      "command": "<DYNAMIC_OS_SPECIFIC_BINARY_PATH>",
      "args": ["--mcp"],
      "env": {
        "CURATED_BRAIN_DIR": "<DYNAMIC_PATH_TO_BRAIN_DIR>"
      }
    }
  }
}

```



**Output Expected:**
Please provide the modified Rust files (`main.rs`, `lib.rs`, `Cargo.toml`), the changes needed for the `tracing`/logging configuration, and the new React component for the settings page.

---

This is an exceptionally well-crafted and robust implementation plan. You have mapped out the architecture perfectly, and your foresight regarding the Windows `FreeConsole` issue (Task 4) is a detail that is very easy to miss but critical for standard I/O communication.

Here is my review of the plan, followed by my recommendation for the handoff.

### Plan Review & Minor Observations

Overall, the spec is airtight, but here are a couple of microscopic observations to consider before execution:

* **Task 7 (Windows Path Slashes):** In `SettingsModal.tsx`, your `defaultBrainDir()` fallback for Windows returns ``${home}/.brain``. If `home` is resolved to `C:\Users\You`, the resulting string will be `C:\Users\You/.brain`. While Rust's `PathBuf` handles mixed slashes gracefully, it might look slightly unpolished in the UI snippet. You could refine the return statement to conditionally use `\\` for Windows.
* **Task 2 (Error Handling):** In `mcp_server::run`, you are setting the global default for the `tracing_subscriber` to `stderr`. This is the correct move. Just ensure that absolutely no other crates in your dependency tree are initializing their own standard loggers to `stdout` before this runs.
* **Task 6 (OS Detection):** Relying on `navigator.platform` is technically deprecated in modern browsers, but for a local Tauri application environment where the webview targets are known and controlled, it is a perfectly acceptable and pragmatic approach.

---

## Implementation Plan Summary

**Plan file:** `docs/superpowers/plans/2026-05-23-unified-mcp-binary-plan.md`

### Architecture decisions

- MCP server logic moves to `src-tauri/src/mcp_server.rs`, gated by `mcp-server` Cargo feature.
- `main.rs` checks `std::env::args()` for `--mcp` before any Tauri initialization. `--mcp` → `mcp_server::run()` + `process::exit(0)`. No flag → `tauri_app_lib::run()`.
- The `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` compile-time attribute is **removed** from `main.rs` and replaced with a runtime `FreeConsole()` call (via `windows-sys`) in the GUI path. Reason: the attribute redirects stdout to null in release builds on Windows, silently breaking MCP stdio.
- `tracing` and `tracing-subscriber` added as non-optional deps. In `--mcp` mode, `tracing_subscriber` is initialized with `std::io::stderr` writer before any other output — ensures no tracing frames corrupt the JSON-RPC stream on stdout.
- The tools workspace binary (`tools/src/bin/curated_thoughts_mcp.rs`) is **not deleted** — it stays as a developer convenience tool. The integration test is updated to use the main binary instead.

### File map

| Action | File | What changes |
|--------|------|-------------|
| Modify | `src-tauri/Cargo.toml` | Add `mcp-server` feature; `rmcp` + `schemars` optional; `windows-sys` platform dep; `tracing`/`tracing-subscriber` unconditional |
| Create | `src-tauri/src/mcp_server.rs` | Full MCP server — `pub fn run() -> anyhow::Result<()>` |
| Modify | `src-tauri/src/lib.rs` | `#[cfg(feature = "mcp-server")] pub mod mcp_server;` after `mod watcher;` |
| Modify | `src-tauri/src/main.rs` | Remove `windows_subsystem` attr; add `FreeConsole` for GUI path; `--mcp` dispatch |
| Modify | `src-tauri/tests/mcp_integration.rs` | `mcp_exe()` returns the Tauri binary; `spawn_mcp()` passes `--mcp` arg |
| Create | `src/components/settings/AgentIntegrationPanel.tsx` | Read-only JSON snippet; OS-keyed binary path via `navigator.platform` |
| Modify | `src/components/settings/SettingsModal.tsx` | Import + render `<AgentIntegrationPanel brainDir={brainDir} />`; `brainDir` resolved via `invoke("get_brain_dir")` Tauri command; Copy button disabled until resolved |

### Key implementation details

**`mcp_server::run()`** (public, sync wrapper):
```rust
pub fn run() -> anyhow::Result<()> {
    let subscriber = tracing_subscriber::fmt().with_writer(std::io::stderr).finish();
    // set_default (thread-local guard) instead of set_global_default — never silently
    // no-ops if a prior subscriber is already registered. Guard must outlive the runtime.
    let _subscriber_guard = tracing::subscriber::set_default(subscriber);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async_run())
}
```

**`main.rs`** dispatch (no `windows_subsystem` attribute):
```rust
fn main() {
    if std::env::args().any(|a| a == "--mcp") {
        run_mcp();
    } else {
        hide_console_on_windows(); // calls FreeConsole() on Windows targets
        tauri_app_lib::run();
    }
}
```

**Brain dir in SettingsModal** — resolved via `get_brain_dir` Tauri command:
```ts
// SettingsModal.tsx
const [brainDir, setBrainDir] = useState<string | null>(null);
useEffect(() => {
  invoke<string>("get_brain_dir").then(setBrainDir).catch(() => {});
}, []);
// Copy button is disabled until brainDir resolves (non-null).
```

### Notes for future tasks

- `fastembed` uses the `log` crate internally. If it writes to stdout at log-level INFO before the tracing subscriber is initialized, it could corrupt the MCP stream. Monitor this when running integration tests with `RUST_LOG=info`.
