#!/usr/bin/env bash
# tools/smoke_test_mcp_sidecar.sh
# Usage: smoke_test_mcp_sidecar.sh <path-to-curated-thoughts-binary>
# Spawns the binary with --mcp and verifies it answers an initialize request.
set -euo pipefail

BIN="${1:?usage: smoke_test_mcp_sidecar.sh <binary>}"
[ -x "$BIN" ] || { echo "FAIL: $BIN is not executable" >&2; exit 1; }

REQ='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke-test","version":"0.0.1"}}}'

RESP=$(printf '%s\n' "$REQ" | timeout 30 "$BIN" --mcp 2>/dev/null | head -n1)

echo "$RESP" | grep -q '"serverInfo"' || {
  echo "FAIL: no serverInfo in response: $RESP" >&2
  exit 1
}
echo "PASS: $BIN answered initialize handshake"
