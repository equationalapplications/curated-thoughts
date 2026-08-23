#!/usr/bin/env bash
# tools/smoke_test_mcp_sidecar.sh
# Usage: smoke_test_mcp_sidecar.sh <path-to-curated-thoughts-binary>
# Spawns the binary with --mcp and verifies it answers an initialize request.
#
# CURATED_BRAIN_CONFIG: the server honors the CURATED_BRAIN_CONFIG environment
# variable to point at a non-default config file. For local runs against a
# non-default config, export it before invoking this script, e.g.:
#   CURATED_BRAIN_CONFIG=/path/to/config.toml ./tools/smoke_test_mcp_sidecar.sh ./target/release/curated-thoughts
#
# Portability: uses no GNU coreutils. The 30-second bound is implemented by
# killing the sidecar after the first response line arrives (or when this
# script exits), so no external `timeout` command is needed — works on macOS.
set -euo pipefail

BIN="${1:?usage: smoke_test_mcp_sidecar.sh <binary>}"
[ -x "$BIN" ] || { echo "FAIL: $BIN is not executable" >&2; exit 1; }

REQ='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke-test","version":"0.0.1"}}}'

RESP_FILE=$(mktemp)
trap 'kill "$SERVER_PID" 2>/dev/null || true; wait "$SERVER_PID" 2>/dev/null || true; rm -f "$RESP_FILE"' EXIT

# Run the sidecar in the background; capture stdout to a file so post-handshake
# output can never trigger SIGPIPE and so we control process lifetime without
# an external `timeout` command. Failures here are NOT masked: the validation
# below exits nonzero if no valid response line was produced.
printf '%s\n' "$REQ" | "$BIN" --mcp >"$RESP_FILE" 2>/dev/null &
SERVER_PID=$!

FIRST_LINE=""
for _ in $(seq 1 300); do # up to ~30s at 0.1s per poll
  FIRST_LINE=$(head -n 1 "$RESP_FILE" 2>/dev/null || true)
  [ -n "$FIRST_LINE" ] && break
  kill -0 "$SERVER_PID" 2>/dev/null || break # server died before responding
  sleep 0.1
done

python3 - "$FIRST_LINE" <<'PYEOF'
import json, sys
line = sys.argv[1]
try:
    obj = json.loads(line)
except Exception:
    print(f"FAIL: first response line is not valid JSON: {line!r}", file=sys.stderr)
    sys.exit(1)
ok = (
    isinstance(obj, dict)
    and obj.get("jsonrpc") == "2.0"
    and obj.get("id") == 1
    and isinstance(obj.get("result"), dict)
    and isinstance(obj["result"].get("serverInfo"), dict)
)
if not ok:
    print(f"FAIL: not a valid initialize response (jsonrpc/id/result.serverInfo): {line!r}", file=sys.stderr)
    sys.exit(1)
print("PASS: sidecar answered initialize handshake")
PYEOF
