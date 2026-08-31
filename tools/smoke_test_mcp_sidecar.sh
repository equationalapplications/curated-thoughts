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

# ── Onboard + Doctor smoke (exercises CLI subcommands before --mcp launch) ────
# Stage into a per-invocation HOME so a real ~/.brain is never touched.
SMOKE_HOME=$(mktemp -d)
export HOME="$SMOKE_HOME"
export CURATED_BRAIN_CONFIG="$SMOKE_HOME/.brain/config.json"

# Canned input for the interactive --onboard prompts:
# Embedding: option 1 (Local Ollama)
# Generation: option 0 (Skip / unconfigured)
# Knowledge schema: option 1 (software-org, the CLI default) — prompt added in
#   PR #124 (0b3bc15); a missing third line hits read_line EOF and aborts
#   onboarding, which broke this smoke test on the v1.38.0 build run.
# Vault path comes from --vault flag below so no stdin required for that.
ONBOARD_INPUT=$'1\n0\n1\n'
if printf '%s' "$ONBOARD_INPUT" | "$BIN" --onboard --vault "$SMOKE_HOME/vault" --force >/dev/null 2>&1; then
    if [ -f "$CURATED_BRAIN_CONFIG" ]; then
        echo "PASS: --onboard wrote config.json"
    else
        echo "FAIL: --onboard did not write config.json" >&2
        exit 1
    fi
    if [ -d "$SMOKE_HOME/vault/immutable-source-files" ] && [ -d "$SMOKE_HOME/vault/wiki" ]; then
        echo "PASS: --onboard created vault layout"
    else
        echo "FAIL: --onboard did not create vault layout" >&2
        exit 1
    fi
else
    echo "FAIL: --onboard exited non-zero" >&2
    exit 1
fi

# Seed a minimal brain.db inside the stub HOME. Since PR #124 the sidecar
# fail-fasts on a missing DB instead of probing the real ~/.brain, so the
# smoke test must be self-contained (same recipe build.yml uses).
mkdir -p "$SMOKE_HOME/.brain"
python3 -c "import sqlite3; sqlite3.connect('$SMOKE_HOME/.brain/brain.db').execute('CREATE TABLE IF NOT EXISTS chunks (id INTEGER PRIMARY KEY)')"

DOCTOR_RC=0
# `|| DOCTOR_RC=$?` keeps the real exit status: with `|| true` the subsequent
# `$?` would always be 0 and a failing --doctor would silently "pass".
DOCTOR_OUT=$("$BIN" --doctor 2>&1) || DOCTOR_RC=$?
if [ "$DOCTOR_RC" -eq 0 ]; then
    echo "PASS: --doctor exit 0"
else
    echo "FAIL: --doctor exit $DOCTOR_RC (expected 0)" >&2
    echo "$DOCTOR_OUT" >&2
    exit 1
fi

REQ='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke-test","version":"0.0.1"}}}'

RESP_FILE=$(mktemp)
trap 'kill "$SERVER_PID" 2>/dev/null || true; wait "$SERVER_PID" 2>/dev/null || true; rm -f "$RESP_FILE"; rm -rf "$SMOKE_HOME"' EXIT

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
