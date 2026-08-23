# MCP Sidecar Bundling Design

**Date:** 2026-08-23
**Status:** Approved design (pending implementation)
**Repo:** curated-thoughts v1.22.0

## Problem

The MCP server (`rmcp`, stdio JSON-RPC) is compiled into the main binary behind the `mcp-server` cargo feature and the `--mcp` flag, but release builds do not enable that feature. Users who install the app from GitHub releases get no MCP server; agents (Claude, Copilot, Hermes, Aider) cannot connect without building from source.

## Goal

Every GitHub release bundle ships a working MCP server as a Tauri sidecar binary, invocable as:

```
<install-dir>/curated-thoughts --mcp
```

## Decision record

- **Approach:** Ship the *main* app binary, built with `--features mcp-server`, as the sidecar. One code path; the sidecar is literally the same code users run. The separate `tools/` crate bin `curated-thoughts-mcp` stays dev-only.
- **Sidecar name:** `curated-thoughts` (same as the app). On disk in the repo it is `src-tauri/binaries/curated-thoughts-<target-triple>` per Tauri's `externalBin` convention; Tauri strips the triple when installing.
- **Windows console flash:** accepted for v1. Agent clients spawn sidecars with `CREATE_NO_WINDOW`; this is standard for MCP servers on Windows. Documented as a known limitation, not worked around.

## Architecture

### Build flow (CI — `.github/workflows/build.yml`)

1. Existing setup steps run unchanged (deps, node, pnpm, rust-toolchain with `targets:` for macOS universal).
2. **New step "Build MCP sidecar":**
   ```
   cargo build --release --manifest-path src-tauri/Cargo.toml --features mcp-server --bin curated-thoughts
   ```
3. **New step "Stage sidecar":** copy the built binary to
   `src-tauri/binaries/curated-thoughts-$TARGET_TRIPLE`
   where `$TARGET_TRIPLE` resolves per matrix leg:
   - ubuntu-22.04 → `x86_64-unknown-linux-gnu`
   - windows-latest → `x86_64-pc-windows-msvc.exe`
   - macos-latest → built twice (`--target aarch64-apple-darwin`, `--target x86_64-apple-darwin`), then merged into a single fat binary with `lipo -create` staged as `curated-thoughts-universal-apple-darwin` — Tauri's `externalBin` lookup under the `universal-apple-darwin` build expects exactly that one file, not the per-slice binaries.

   Staged sidecars are build artifacts (not committed); implementation must create `src-tauri/binaries/` with a `.gitignore` covering `curated-thoughts-*` (the directory does not exist today).
4. `tauri-action` runs unchanged otherwise; `tauri.conf.json` now contains `"bundle": { "externalBin": ["binaries/curated-thoughts"] }`.

### Runtime contract (unchanged)

- Stdio JSON-RPC only; tracing to stderr; stdout carries protocol traffic exclusively (`run_mcp()` already guarantees this).
- Config via `CURATED_BRAIN_DIR` env or default `~/.brain`. Local stdio = trust boundary; no network listener is opened.
- Windows: `--mcp` routing happens before any `FreeConsole()` GUI-mode handling, so stdio is unaffected by existing console logic.

## Testing

1. **Local smoke test (any OS):** build the sidecar, spawn it, write an `initialize` JSON-RPC request to stdin, assert the response on stdout parses and carries `serverInfo`.
2. **CI smoke test (ubuntu leg only):** after `tauri-action`, install the `.deb`/AppImage output or run the staged sidecar directly, repeat the `initialize` handshake, fail the job if no valid response.

## Documentation

- README: new "Connecting AI agents" section — command line above, pointer to `specs/curated-thoughts-mcp-coding-spec.md` for tool inventory.
- Note that `curated-thoughts-mcp` (tools crate) remains the development/manual path.

## Out of scope (v1)

- Settings UI exposing the connection command.
- Changes to the `tools/` crate beyond leaving it untouched.
- Network/WebSocket MCP transports.

## Known limitations

- Windows console flash unless client passes `CREATE_NO_WINDOW`.
- Sidecar doubles installed size (~binary duplicated inside bundle).
