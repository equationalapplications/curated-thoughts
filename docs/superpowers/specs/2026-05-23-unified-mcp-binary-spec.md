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