#!/usr/bin/env bash
# tools/smoke_test_mcp_sidecar.sh
# Usage: smoke_test_mcp_sidecar.sh <path-to-curated-thoughts-binary>
# Spawns the binary with --mcp and verifies it answers an initialize request.
#
# CURATED_BRAIN_CONFIG: the server honors the CURATED_BRAIN_CONFIG environment
# variable to point at a non-default config file. For local runs against a
# non-default config, export it before invoking this script, e.g.:
#   CURATED_BRAIN_CONFIG=/path/to/config.toml ./tools/smoke_test_mcp_sidecar.sh ./target/release/curated-thoughts
set -euo pipefail

BIN="${1:?usage: smoke_test_mcp_sidecar.sh <binary>}"
[ -x "$BIN" ] || { echo "FAIL: $BIN is not executable" >&2; exit 1; }

REQ='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke-test","version":"0.0.1"}}}'

# Capture full stdout, then take line 1 with `read` (no head-in-pipeline).
# Both stages are status-neutralized: once the handshake line arrives the
# server may sleep forever (timeout reaps it -> 124) or spew output that
# triggers SIGPIPE (141); neither may fail the gate. A server that never
# responds leaves RESP empty, so FIRST_LINE is empty and grep fails below.
RESP=$(printf '%s\n' "$REQ" | timeout 30 "$BIN" --mcp 2>/dev/null || true)
IFS= read -r FIRST_LINE <<<"$RESP" || true

echo "$FIRST_LINE" | grep -q '"serverInfo"' || {
  echo "FAIL: no serverInfo in response: $FIRST_LINE" >&2
  exit 1
}
echo "PASS: $BIN answered initialize handshake"
